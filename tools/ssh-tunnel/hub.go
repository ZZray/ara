// Copyright 2026 Ray Zhang.
package main

import (
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"log"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/coder/websocket"
	"golang.org/x/crypto/ssh"
)

type peer struct {
	conn    *ssh.ServerConn
	key     ssh.PublicKey
	role    string
	device  *hubDevice
	streams chan struct{}
}
type hubDevice struct {
	info            deviceInfo
	peer            *peer
	owner           *peer
	codeHash        [32]byte
	expires         time.Time
	ready           bool
	expiry          *time.Timer
	streams         chan struct{}
	directAttempted bool
}
type hub struct {
	mu      sync.Mutex
	host    ssh.Signer
	devices map[string]*hubDevice
	codes   map[[32]byte]*hubDevice
	peers   map[*peer]struct{}
	slots   chan struct{}
	pairTTL time.Duration
	closed  bool
}

func newHub() (*hub, error) {
	host, _, err := newSigner()
	if err != nil {
		return nil, err
	}
	return &hub{host: host, devices: map[string]*hubDevice{}, codes: map[[32]byte]*hubDevice{}, peers: map[*peer]struct{}{}, slots: make(chan struct{}, 128), pairTTL: 10 * time.Minute}, nil
}

func (h *hub) Close() {
	h.mu.Lock()
	h.closed = true
	peers := make([]*peer, 0, len(h.peers))
	for p := range h.peers {
		peers = append(peers, p)
	}
	h.mu.Unlock()
	for _, p := range peers {
		p.conn.Close()
	}
}

func (h *hub) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", 405)
		return
	}
	switch r.URL.Path {
	case "/healthz":
		w.Header().Set("Content-Type", "text/plain")
		fmt.Fprintln(w, "ry-ssh-tunnel ok")
	case "/host-key":
		w.Header().Set("Cache-Control", "no-store")
		w.Header().Set("Content-Type", "text/plain")
		w.Write(ssh.MarshalAuthorizedKey(h.host.PublicKey()))
	case "/connect":
		h.accept(w, r)
	default:
		http.NotFound(w, r)
	}
}

func (h *hub) accept(w http.ResponseWriter, r *http.Request) {
	// A browser must not be able to create an ambient session through this endpoint.
	if r.Header.Get("Origin") != "" {
		http.Error(w, "browser connections are not supported", 403)
		return
	}
	select {
	case h.slots <- struct{}{}:
		defer func() { <-h.slots }()
	default:
		http.Error(w, "connection capacity reached", 503)
		return
	}
	ws, err := websocket.Accept(w, r, nil)
	if err != nil {
		return
	}
	defer ws.CloseNow()
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	nc := websocket.NetConn(ctx, ws, websocket.MessageBinary)
	defer nc.Close()
	nc.SetDeadline(time.Now().Add(handshakeTimeout))
	var authenticated ssh.PublicKey
	config := &ssh.ServerConfig{MaxAuthTries: 2, PublicKeyCallback: func(c ssh.ConnMetadata, key ssh.PublicKey) (*ssh.Permissions, error) {
		if (c.User() != "device" && c.User() != "controller") || key.Type() != ssh.KeyAlgoED25519 {
			return nil, errors.New("unsupported role or key")
		}
		authenticated = key
		return &ssh.Permissions{}, nil
	}}
	config.AddHostKey(h.host)
	conn, channels, requests, err := ssh.NewServerConn(nc, config)
	if err != nil {
		return
	}
	defer conn.Close()
	nc.SetDeadline(time.Time{})
	p := &peer{conn: conn, key: authenticated, role: conn.User(), streams: make(chan struct{}, maxStreams)}
	h.mu.Lock()
	if h.closed {
		h.mu.Unlock()
		return
	}
	h.peers[p] = struct{}{}
	h.mu.Unlock()
	defer h.removePeer(p)
	// Unregistered devices cannot occupy a slot forever. Controller sessions have a
	// finite support-window too; no background enrolment or durable access exists.
	lifetime := 10 * time.Minute
	if p.role == "device" {
		lifetime = 30 * time.Second
	}
	expiry := time.AfterFunc(lifetime, func() { conn.Close() })
	defer expiry.Stop()
	var workers sync.WaitGroup
	defer func() { cancel(); conn.Close(); workers.Wait() }()
	workers.Go(func() {
		conn.Wait()
		cancel()
		// Do not wait for an in-progress pairing request to finish before revoking
		// its device. In particular, disconnect during local authorization cancels
		// the device's file wait and lets the controlled-side script clean up.
		h.removePeer(p)
	})
	workers.Go(func() {
		for ch := range channels {
			if p.role != "controller" || (ch.ChannelType() != "ry-forward" && ch.ChannelType() != directProtocol) {
				ch.Reject(ssh.Prohibited, "unsupported channel")
				continue
			}
			select {
			case p.streams <- struct{}{}:
				workers.Go(func() {
					defer func() { <-p.streams }()
					if ch.ChannelType() == directProtocol {
						h.signal(ctx, p, ch)
					} else {
						h.forward(ctx, p, ch)
					}
				})
			default:
				ch.Reject(ssh.ResourceShortage, "stream limit")
			}
		}
	})
	for req := range requests {
		var response []byte
		var requestErr error
		switch {
		case p.role == "device" && req.Type == "ry-register":
			response, requestErr = h.register(p, req.Payload)
			if requestErr == nil {
				expiry.Stop()
			}
		case p.role == "controller" && req.Type == "ry-pair":
			response, requestErr = h.pair(p, req.Payload)
			if requestErr == nil {
				expiry.Reset(12 * time.Hour)
			}
		case p.role == "controller" && req.Type == "ry-list":
			response = h.list(p)
		case p.role == "controller" && req.Type == "ry-stop":
			requestErr = h.stop(p, req.Payload)
		default:
			requestErr = errors.New("unsupported request")
		}
		if requestErr != nil {
			response = []byte(requestErr.Error())
		}
		if err := req.Reply(requestErr == nil, response); err != nil {
			return
		}
	}
}

func (h *hub) register(p *peer, payload []byte) ([]byte, error) {
	var input struct {
		Name string `json:"name"`
	}
	if decode(payload, &input) != nil || input.Name == "" || len(input.Name) > 128 || strings.ContainsAny(input.Name, "\r\n\x00") {
		return nil, errors.New("invalid device name")
	}
	id, err := randomID(12)
	if err != nil {
		return nil, err
	}
	code, err := randomID(16)
	if err != nil {
		return nil, err
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if _, connected := h.peers[p]; !connected || h.closed {
		return nil, errors.New("device connection already closed")
	}
	if p.device != nil {
		return nil, errors.New("already registered")
	}
	d := &hubDevice{info: deviceInfo{DeviceID: id, Name: input.Name}, peer: p, codeHash: sha256.Sum256([]byte(code)), expires: time.Now().Add(h.pairTTL), streams: make(chan struct{}, maxStreams)}
	h.devices[id] = d
	h.codes[d.codeHash] = d
	p.device = d
	d.expiry = time.AfterFunc(h.pairTTL, func() {
		h.mu.Lock()
		expired := h.devices[id] == d && d.owner == nil
		h.mu.Unlock()
		if expired {
			p.conn.Close()
		}
	})
	return encode(pairing{DeviceID: id, Code: code, ExpiresAt: d.expires}), nil
}

func (h *hub) pair(owner *peer, payload []byte) ([]byte, error) {
	var input struct {
		Code string `json:"code"`
	}
	if decode(payload, &input) != nil || len(input.Code) != 26 {
		return nil, errors.New("invalid or expired pairing code")
	}
	hash := sha256.Sum256([]byte(input.Code))
	h.mu.Lock()
	if _, connected := h.peers[owner]; !connected || h.closed {
		h.mu.Unlock()
		return nil, errors.New("controller connection already closed")
	}
	d := h.codes[hash]
	if d == nil || d.owner != nil || !time.Now().Before(d.expires) {
		h.mu.Unlock()
		return nil, errors.New("invalid or expired pairing code")
	}
	// Reservation and single use are atomic. The authenticated SSH key is also the
	// inner login key; the caller cannot substitute another user's public key.
	delete(h.codes, hash)
	d.owner = owner
	d.expiry.Stop()
	h.mu.Unlock()
	response, err := request(d.peer.conn, "ry-authorize", ssh.MarshalAuthorizedKey(owner.key), authorizationTimeout+10*time.Second)
	if err != nil {
		d.peer.conn.Close()
		return nil, errors.New("device authorization failed; restart the controlled session to retry")
	}
	var auth authorization
	if decode(response, &auth) != nil || !auth.OK || validateTargets(auth.Targets) != nil {
		d.peer.conn.Close()
		return nil, errors.New("device did not supply valid authorization and host keys")
	}
	h.mu.Lock()
	if h.devices[d.info.DeviceID] != d || h.closed {
		h.mu.Unlock()
		d.peer.conn.Close()
		return nil, errors.New("device disconnected during authorization")
	}
	if _, ok := h.peers[owner]; !ok {
		h.mu.Unlock()
		d.peer.conn.Close()
		return nil, errors.New("controller disconnected during authorization")
	}
	d.info.Targets = auth.Targets
	d.ready = true
	info := d.info
	h.mu.Unlock()
	log.Printf("paired device=%s", d.info.DeviceID)
	return encode(info), nil
}

func (h *hub) list(owner *peer) []byte {
	h.mu.Lock()
	defer h.mu.Unlock()
	result := make([]deviceInfo, 0)
	for _, d := range h.devices {
		if d.owner == owner && d.ready {
			result = append(result, d.info)
		}
	}
	return encode(result)
}

func (h *hub) stop(owner *peer, payload []byte) error {
	var input struct {
		DeviceID string `json:"device_id"`
	}
	if decode(payload, &input) != nil {
		return errors.New("invalid stop request")
	}
	h.mu.Lock()
	d := h.devices[input.DeviceID]
	allowed := d != nil && d.owner == owner
	h.mu.Unlock()
	if !allowed {
		return errors.New("device is not owned by this controller")
	}
	d.peer.conn.Close()
	return nil
}

func (h *hub) removePeer(p *peer) {
	h.mu.Lock()
	delete(h.peers, p)
	closePeers := []*peer{}
	if d := p.device; d != nil {
		delete(h.devices, d.info.DeviceID)
		delete(h.codes, d.codeHash)
		d.expiry.Stop()
	}
	if p.role == "controller" {
		for _, d := range h.devices {
			if d.owner == p {
				closePeers = append(closePeers, d.peer)
			}
		}
	}
	h.mu.Unlock()
	for _, other := range closePeers {
		other.conn.Close()
	}
}

func (h *hub) forward(ctx context.Context, p *peer, in ssh.NewChannel) {
	var input openRequest
	if decode(in.ExtraData(), &input) != nil {
		in.Reject(ssh.Prohibited, "invalid request")
		return
	}
	h.mu.Lock()
	d := h.devices[input.DeviceID]
	allowed := d != nil && d.owner == p && d.ready
	if allowed {
		_, allowed = d.info.Targets[input.Target]
	}
	h.mu.Unlock()
	if !allowed {
		in.Reject(ssh.Prohibited, "target unavailable for this controller")
		return
	}
	select {
	case d.streams <- struct{}{}:
		defer func() { <-d.streams }()
	default:
		in.Reject(ssh.ResourceShortage, "device stream limit")
		return
	}
	timer := time.AfterFunc(handshakeTimeout, func() { d.peer.conn.Close() })
	out, requests, err := d.peer.conn.OpenChannel("ry-target", []byte(input.Target))
	timer.Stop()
	if err != nil {
		in.Reject(ssh.ConnectionFailed, "target connection failed")
		return
	}
	defer out.Close()
	go ssh.DiscardRequests(requests)
	channel, requests, err := in.Accept()
	if err != nil {
		return
	}
	go ssh.DiscardRequests(requests)
	copyStreams(ctx, channel, out)
}
