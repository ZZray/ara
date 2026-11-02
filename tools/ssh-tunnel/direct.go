// Copyright 2026 Ray Zhang.
package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"sync"
	"time"

	"github.com/pion/datachannel"
	"github.com/pion/webrtc/v4"
	"golang.org/x/crypto/ssh"
)

const directProtocol = "ry-direct-v1"
const directTimeout = 15 * time.Second
const maxSignal = 64 * 1024
const directMessageSize = 16 * 1024

type directConfig struct {
	disabled bool
	stun     []string
	loopback bool // controlled tests only; never enabled by a CLI option
}
type directConfigKey struct{}

func directSettings(ctx context.Context) directConfig {
	if cfg, ok := ctx.Value(directConfigKey{}).(directConfig); ok {
		return cfg
	}
	return directConfig{stun: []string{"stun:stun.cloudflare.com:3478", "stun:stun.l.google.com:19302"}}
}

type directSignal struct {
	Description webrtc.SessionDescription `json:"description"`
	HostKey     string                    `json:"host_key,omitempty"`
}

// Each signaling channel carries exactly one bounded JSON document each way.
// Closing it cancels negotiation without closing the relay control connection.
func readSignal(r io.Reader) (directSignal, error) {
	data, err := io.ReadAll(io.LimitReader(r, maxSignal+1))
	if err != nil {
		return directSignal{}, err
	}
	var signal directSignal
	if err = decode(data, &signal); err != nil {
		return signal, err
	}
	return signal, nil
}

func writeSignal(ch ssh.Channel, signal directSignal) error {
	data := encode(signal)
	if len(data) > maxSignal {
		return errors.New("direct signaling message too large")
	}
	if _, err := ch.Write(data); err != nil {
		return err
	}
	return ch.CloseWrite()
}

// DataChannels carry messages, while SSH needs a byte stream. Preserve unread
// bytes across small SSH reads and bound messages and SCTP's pending writes.
type dataConn struct {
	datachannel.ReadWriteCloserDeadliner
	readMu, writeMu sync.Mutex
	pending         []byte
	buffer          [directMessageSize]byte
}

func (c *dataConn) Read(p []byte) (int, error) {
	c.readMu.Lock()
	defer c.readMu.Unlock()
	if len(p) == 0 {
		return 0, nil
	}
	for len(c.pending) == 0 {
		n, err := c.ReadWriteCloserDeadliner.Read(c.buffer[:])
		if err != nil {
			return 0, err
		}
		c.pending = c.buffer[:n]
	}
	n := copy(p, c.pending)
	c.pending = c.pending[n:]
	return n, nil
}

func (c *dataConn) Write(p []byte) (int, error) {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	total := 0
	for len(p) > 0 {
		size := min(len(p), directMessageSize)
		n, err := c.ReadWriteCloserDeadliner.Write(p[:size])
		total += n
		if err != nil {
			return total, err
		}
		if n != size {
			return total, io.ErrShortWrite
		}
		p = p[n:]
	}
	return total, nil
}

type directAddr string

func (a directAddr) Network() string     { return "webrtc" }
func (a directAddr) String() string      { return string(a) }
func (c *dataConn) LocalAddr() net.Addr  { return directAddr("local") }
func (c *dataConn) RemoteAddr() net.Addr { return directAddr("peer") }
func (c *dataConn) SetDeadline(t time.Time) error {
	return errors.Join(c.SetReadDeadline(t), c.SetWriteDeadline(t))
}

type directPeer struct {
	pc     *webrtc.PeerConnection
	opened chan *dataConn
	stop   func() bool
	once   sync.Once
}

func newDirectPeer(ctx context.Context, offerer bool) (*directPeer, error) {
	cfg := directSettings(ctx)
	if cfg.disabled {
		return nil, errors.New("direct transport disabled")
	}
	settings := webrtc.SettingEngine{}
	settings.DetachDataChannels()
	settings.EnableDataChannelBlockWrite(true)
	settings.SetSCTPMaxMessageSize(directMessageSize)
	settings.SetNetworkTypes([]webrtc.NetworkType{webrtc.NetworkTypeUDP4, webrtc.NetworkTypeUDP6})
	settings.SetIncludeLoopbackCandidate(cfg.loopback)
	settings.SetSTUNGatherTimeout(2 * time.Second)
	settings.SetICETimeouts(5*time.Second, 10*time.Second, 2*time.Second)
	config := webrtc.Configuration{}
	if len(cfg.stun) > 0 {
		config.ICEServers = []webrtc.ICEServer{{URLs: cfg.stun}}
	}
	pc, err := webrtc.NewAPI(webrtc.WithSettingEngine(settings)).NewPeerConnection(config)
	if err != nil {
		return nil, err
	}
	p := &directPeer{pc: pc, opened: make(chan *dataConn, 1)}
	p.stop = context.AfterFunc(ctx, func() { pc.Close() })
	pc.OnConnectionStateChange(func(state webrtc.PeerConnectionState) {
		if state == webrtc.PeerConnectionStateFailed {
			go p.Close()
		}
	})
	attach := func(dc *webrtc.DataChannel) {
		dc.OnOpen(func() {
			if dc.Label() != directProtocol || !dc.Ordered() || dc.MaxRetransmits() != nil || dc.MaxPacketLifeTime() != nil {
				dc.Close()
				return
			}
			raw, err := dc.DetachWithDeadline()
			if err != nil {
				pc.Close()
				return
			}
			select {
			case p.opened <- &dataConn{ReadWriteCloserDeadliner: raw}:
			default:
				raw.Close()
			}
		})
	}
	if offerer {
		dc, err := pc.CreateDataChannel(directProtocol, nil)
		if err != nil {
			p.Close()
			return nil, err
		}
		attach(dc)
	} else {
		pc.OnDataChannel(attach)
	}
	return p, nil
}

func (p *directPeer) Close() {
	p.once.Do(func() { p.stop(); p.pc.Close() })
}

func (p *directPeer) description(ctx context.Context, remote *webrtc.SessionDescription) (webrtc.SessionDescription, error) {
	var desc webrtc.SessionDescription
	var err error
	if remote == nil {
		desc, err = p.pc.CreateOffer(nil)
	} else {
		if remote.Type != webrtc.SDPTypeOffer {
			return desc, errors.New("expected a direct offer")
		}
		if err = p.pc.SetRemoteDescription(*remote); err != nil {
			return desc, err
		}
		desc, err = p.pc.CreateAnswer(nil)
	}
	if err != nil {
		return desc, err
	}
	gathered := webrtc.GatheringCompletePromise(p.pc)
	if err = p.pc.SetLocalDescription(desc); err != nil {
		return desc, err
	}
	select {
	case <-ctx.Done():
		return desc, ctx.Err()
	case <-gathered:
		return *p.pc.LocalDescription(), nil
	}
}

type directSession struct {
	peer *directPeer
	conn ssh.Conn
	done chan struct{}
	path string
}

func (c *controller) tryDirect(info deviceInfo) error {
	ctx, cancel := context.WithTimeout(c.ctx, directTimeout)
	defer cancel()
	p, err := newDirectPeer(c.ctx, true)
	if err != nil {
		return err
	}
	ready := false
	defer func() {
		if !ready {
			p.Close()
		}
	}()
	desc, err := p.description(ctx, nil)
	if err != nil {
		return err
	}
	// OpenChannel can itself wait for an old/unresponsive hub. Return on the
	// deadline and close any late channel, without poisoning the relay session.
	type opened struct {
		ch  ssh.Channel
		err error
	}
	result := make(chan opened)
	go func() {
		ch, requests, err := c.transport.Conn.OpenChannel(directProtocol, encode(openRequest{DeviceID: info.DeviceID}))
		if err == nil {
			go ssh.DiscardRequests(requests)
		}
		select {
		case result <- opened{ch, err}:
		case <-ctx.Done():
			if ch != nil {
				ch.Close()
			}
		}
	}()
	var ch ssh.Channel
	select {
	case <-ctx.Done():
		return ctx.Err()
	case r := <-result:
		if r.err != nil {
			return r.err
		}
		ch = r.ch
	}
	defer ch.Close()
	stop := context.AfterFunc(ctx, func() { ch.Close(); p.pc.Close() })
	defer stop()
	if err = writeSignal(ch, directSignal{Description: desc}); err != nil {
		return err
	}
	answer, err := readSignal(ch)
	if err != nil {
		return err
	}
	host, err := publicKey(answer.HostKey)
	if err != nil {
		return err
	}
	if answer.Description.Type != webrtc.SDPTypeAnswer {
		return errors.New("expected a direct answer")
	}
	if err = p.pc.SetRemoteDescription(answer.Description); err != nil {
		return err
	}
	var nc *dataConn
	select {
	case nc = <-p.opened:
	case <-ctx.Done():
		return ctx.Err()
	}
	deadline, _ := ctx.Deadline()
	nc.SetDeadline(deadline)
	conn, channels, requests, err := ssh.NewClientConn(nc, info.DeviceID, &ssh.ClientConfig{User: "controller", Auth: []ssh.AuthMethod{ssh.PublicKeys(c.signer)}, HostKeyAlgorithms: []string{ssh.KeyAlgoED25519}, HostKeyCallback: ssh.FixedHostKey(host)})
	if err != nil {
		return err
	}
	nc.SetDeadline(time.Time{})
	go ssh.DiscardRequests(requests)
	go func() {
		for ch := range channels {
			ch.Reject(ssh.Prohibited, "unsupported channel")
		}
	}()
	pair, err := p.pc.SCTP().Transport().ICETransport().GetSelectedCandidatePair()
	if err != nil || pair == nil {
		conn.Close()
		return errors.New("selected ICE pair unavailable")
	}
	if pair.Local.Typ == webrtc.ICECandidateTypeRelay || pair.Remote.Typ == webrtc.ICECandidateTypeRelay {
		conn.Close()
		return errors.New("unexpected relayed ICE candidate")
	}
	s := &directSession{peer: p, conn: conn, done: make(chan struct{}), path: fmt.Sprintf("direct UDP (%s/%s)", pair.Local.Typ, pair.Remote.Typ)}
	if ctx.Err() != nil {
		conn.Close()
		return ctx.Err()
	}
	c.direct.Store(info.DeviceID, s)
	go func() { conn.Wait(); close(s.done); p.Close() }()
	ready = true
	log.Printf("PATH device=%s %s; SSH/file data bypass hub", info.DeviceID, s.path)
	return nil
}

func (h *hub) signal(ctx context.Context, owner *peer, incoming ssh.NewChannel) {
	var input openRequest
	if decode(incoming.ExtraData(), &input) != nil || input.Target != "" {
		incoming.Reject(ssh.Prohibited, "invalid direct request")
		return
	}
	h.mu.Lock()
	d := h.devices[input.DeviceID]
	allowed := d != nil && d.owner == owner && d.ready
	h.mu.Unlock()
	if !allowed {
		incoming.Reject(ssh.Prohibited, "device is not owned by this controller")
		return
	}
	in, requests, err := incoming.Accept()
	if err != nil {
		return
	}
	defer in.Close()
	go ssh.DiscardRequests(requests)
	ctx, cancel := context.WithTimeout(ctx, directTimeout)
	defer cancel()
	stop := context.AfterFunc(ctx, func() { in.Close() })
	defer stop()
	offer, err := readSignal(in)
	if err != nil || offer.HostKey != "" || offer.Description.Type != webrtc.SDPTypeOffer {
		return
	}
	// One attempted negotiation per paired device bounds outstanding channel
	// opens even when an old or unresponsive endpoint never answers the open.
	h.mu.Lock()
	if d.directAttempted {
		h.mu.Unlock()
		return
	}
	d.directAttempted = true
	h.mu.Unlock()
	type opened struct {
		ch  ssh.Channel
		err error
	}
	result := make(chan opened)
	go func() {
		ch, requests, err := d.peer.conn.OpenChannel(directProtocol, nil)
		if err == nil {
			go ssh.DiscardRequests(requests)
		}
		select {
		case result <- opened{ch, err}:
		case <-ctx.Done():
			if ch != nil {
				ch.Close()
			}
		}
	}()
	var out ssh.Channel
	select {
	case <-ctx.Done():
		return
	case r := <-result:
		if r.err != nil {
			return
		}
		out = r.ch
	}
	defer out.Close()
	stopOut := context.AfterFunc(ctx, func() { out.Close() })
	defer stopOut()
	if err = writeSignal(out, offer); err != nil {
		return
	}
	answer, err := readSignal(out)
	if err != nil || answer.Description.Type != webrtc.SDPTypeAnswer {
		return
	}
	answer.HostKey = string(ssh.MarshalAuthorizedKey(d.peer.key))
	writeSignal(in, answer)
}

func serveDirect(ctx context.Context, incoming ssh.NewChannel, signer ssh.Signer, owner ssh.PublicKey, serve func(ssh.NewChannel)) {
	ch, requests, err := incoming.Accept()
	if err != nil {
		return
	}
	defer ch.Close()
	go ssh.DiscardRequests(requests)
	setup, cancel := context.WithTimeout(ctx, directTimeout)
	defer cancel()
	stopChannel := context.AfterFunc(setup, func() { ch.Close() })
	defer stopChannel()
	offer, err := readSignal(ch)
	if err != nil || offer.HostKey != "" {
		return
	}
	p, err := newDirectPeer(ctx, false)
	if err != nil {
		return
	}
	defer p.Close()
	stopPeer := context.AfterFunc(setup, func() { p.pc.Close() })
	defer stopPeer()
	answer, err := p.description(setup, &offer.Description)
	if err != nil {
		return
	}
	if err = writeSignal(ch, directSignal{Description: answer}); err != nil {
		return
	}
	var nc *dataConn
	select {
	case nc = <-p.opened:
	case <-setup.Done():
		return
	}
	deadline, _ := setup.Deadline()
	nc.SetDeadline(deadline)
	config := &ssh.ServerConfig{MaxAuthTries: 2, PublicKeyCallback: func(meta ssh.ConnMetadata, key ssh.PublicKey) (*ssh.Permissions, error) {
		if meta.User() != "controller" || !bytes.Equal(key.Marshal(), owner.Marshal()) {
			return nil, errors.New("unpaired direct identity")
		}
		return &ssh.Permissions{}, nil
	}}
	config.AddHostKey(signer)
	conn, channels, requests, err := ssh.NewServerConn(nc, config)
	if err != nil {
		return
	}
	defer conn.Close()
	nc.SetDeadline(time.Time{})
	stopPeer()
	go ssh.DiscardRequests(requests)
	for incoming := range channels {
		serve(incoming)
	}
}
