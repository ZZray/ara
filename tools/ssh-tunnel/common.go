// Copyright 2026 Ray Zhang.
package main

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base32"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/coder/websocket"
	"golang.org/x/crypto/ssh"
)

const defaultServer = "https://nmrkit.ciqtek.com/ssh-tunnel"
const handshakeTimeout = 10 * time.Second
const authorizationTimeout = 3 * time.Minute
const maxStreams = 16

type target struct {
	Address string `json:"address"`
	User    string `json:"user"`
	HostKey string `json:"host_key"`
}
type authorization struct {
	OK      bool              `json:"ok"`
	Error   string            `json:"error,omitempty"`
	Targets map[string]target `json:"targets,omitempty"`
}
type pairing struct {
	DeviceID  string    `json:"device_id"`
	Code      string    `json:"code"`
	ExpiresAt time.Time `json:"expires_at"`
}
type deviceInfo struct {
	DeviceID string            `json:"device_id"`
	Name     string            `json:"name"`
	Targets  map[string]target `json:"targets,omitempty"`
}
type openRequest struct {
	DeviceID string `json:"device_id"`
	Target   string `json:"target"`
}

func newSigner() (ssh.Signer, ed25519.PrivateKey, error) {
	_, key, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return nil, nil, err
	}
	signer, err := ssh.NewSignerFromKey(key)
	return signer, key, err
}

func randomID(n int) (string, error) {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(b), nil
}

func decode(data []byte, value any) error {
	if len(data) > 64*1024 {
		return errors.New("message too large")
	}
	d := json.NewDecoder(bytes.NewReader(bytes.TrimPrefix(data, []byte{0xef, 0xbb, 0xbf})))
	d.DisallowUnknownFields()
	if err := d.Decode(value); err != nil {
		return err
	}
	if d.Decode(new(any)) != io.EOF {
		return errors.New("expected one JSON value")
	}
	return nil
}

func encode(value any) []byte { b, _ := json.Marshal(value); return b }

func publicKey(line string) (ssh.PublicKey, error) {
	key, _, options, rest, err := ssh.ParseAuthorizedKey([]byte(line))
	if err != nil || len(options) != 0 || len(bytes.TrimSpace(rest)) != 0 {
		return nil, errors.New("invalid public key")
	}
	if key.Type() != ssh.KeyAlgoED25519 {
		return nil, errors.New("only Ed25519 keys are supported")
	}
	return key, nil
}

func validateTargets(targets map[string]target) error {
	if len(targets) < 1 || len(targets) > 2 {
		return errors.New("expected jump and optional backend targets")
	}
	if _, ok := targets["jump"]; !ok {
		return errors.New("jump target is required")
	}
	for name, t := range targets {
		if name != "jump" && name != "backend" {
			return errors.New("unsupported target name")
		}
		host, port, err := net.SplitHostPort(t.Address)
		if err != nil || host == "" || port == "" {
			return fmt.Errorf("%s: invalid target address", name)
		}
		portNumber, err := strconv.Atoi(port)
		if err != nil || portNumber < 1 || portNumber > 65535 {
			return fmt.Errorf("%s: invalid target port", name)
		}
		if t.User == "" || len(t.User) > 128 || strings.ContainsAny(t.User, "\r\n\x00") {
			return fmt.Errorf("%s: invalid login user", name)
		}
		if _, err := publicKey(t.HostKey); err != nil {
			return fmt.Errorf("%s: %w", name, err)
		}
	}
	return nil
}

// User URLs must be HTTPS. The HTTP exception is private to controlled tests.
func serverURL(raw string, allowHTTP bool) (*url.URL, error) {
	u, err := url.Parse(strings.TrimRight(raw, "/"))
	if err != nil || u.Host == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" {
		return nil, errors.New("server must be an HTTPS URL without credentials, query or fragment")
	}
	if u.Scheme != "https" && !(allowHTTP && u.Scheme == "http") {
		return nil, errors.New("server must use HTTPS")
	}
	return u, nil
}

type transport struct {
	Conn     ssh.Conn
	Channels <-chan ssh.NewChannel
	Requests <-chan *ssh.Request
	cancel   context.CancelFunc
}

func (t *transport) Close() error { t.cancel(); return t.Conn.Close() }

func dialTransport(ctx context.Context, server, role string, signer ssh.Signer, hc *http.Client, allowHTTP bool) (*transport, error) {
	u, err := serverURL(server, allowHTTP)
	if err != nil {
		return nil, err
	}
	if hc == nil {
		hc = &http.Client{Timeout: handshakeTimeout, CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("relay redirects are not allowed") }}
	}
	requestCtx, requestCancel := context.WithTimeout(ctx, handshakeTimeout)
	defer requestCancel()
	req, err := http.NewRequestWithContext(requestCtx, http.MethodGet, u.String()+"/host-key", nil)
	if err != nil {
		return nil, err
	}
	resp, err := hc.Do(req)
	if err != nil {
		return nil, fmt.Errorf("relay identity: %w", err)
	}
	data, readErr := io.ReadAll(io.LimitReader(resp.Body, 4097))
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK || readErr != nil || len(data) > 4096 {
		return nil, errors.New("relay identity endpoint failed")
	}
	hostKey, err := publicKey(string(data))
	if err != nil {
		return nil, err
	}
	wsURL := *u
	wsURL.Path = strings.TrimRight(wsURL.Path, "/") + "/connect"
	if wsURL.Scheme == "https" {
		wsURL.Scheme = "wss"
	} else {
		wsURL.Scheme = "ws"
	}
	ws, _, err := websocket.Dial(requestCtx, wsURL.String(), &websocket.DialOptions{HTTPClient: hc})
	if err != nil {
		return nil, fmt.Errorf("relay websocket: %w", err)
	}
	lifetime, cancel := context.WithCancel(ctx)
	nc := websocket.NetConn(lifetime, ws, websocket.MessageBinary)
	nc.SetDeadline(time.Now().Add(handshakeTimeout))
	cc, channels, requests, err := ssh.NewClientConn(nc, u.Host, &ssh.ClientConfig{User: role, Auth: []ssh.AuthMethod{ssh.PublicKeys(signer)}, HostKeyAlgorithms: []string{ssh.KeyAlgoED25519}, HostKeyCallback: ssh.FixedHostKey(hostKey)})
	if err != nil {
		cancel()
		nc.Close()
		return nil, fmt.Errorf("relay SSH handshake: %w", err)
	}
	nc.SetDeadline(time.Time{})
	t := &transport{Conn: cc, Channels: channels, Requests: requests, cancel: cancel}
	context.AfterFunc(lifetime, func() { cc.Close(); ws.CloseNow() })
	go func() {
		ticker := time.NewTicker(20 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-lifetime.Done():
				return
			case <-ticker.C:
				pingCtx, pingCancel := context.WithTimeout(lifetime, 10*time.Second)
				err := ws.Ping(pingCtx)
				pingCancel()
				if err != nil {
					t.Close()
					return
				}
			}
		}
	}()
	return t, nil
}

func request(conn ssh.Conn, kind string, payload []byte, timeout time.Duration) ([]byte, error) {
	timer := time.AfterFunc(timeout, func() { conn.Close() })
	defer timer.Stop()
	ok, response, err := conn.SendRequest(kind, true, payload)
	if err != nil {
		return nil, err
	}
	if !ok {
		return nil, fmt.Errorf("%s rejected: %s", kind, string(response))
	}
	return response, nil
}

// Retains the old ry-ssh-tunnel bridge's half-close and cancellation behavior.
func copyStreams(ctx context.Context, a, b io.ReadWriteCloser) error {
	defer a.Close()
	defer b.Close()
	stop := context.AfterFunc(ctx, func() { a.Close(); b.Close() })
	defer stop()
	done := make(chan error, 2)
	copyOne := func(dst, src io.ReadWriteCloser) {
		_, err := io.Copy(dst, src)
		if half, ok := dst.(interface{ CloseWrite() error }); ok {
			if e := half.CloseWrite(); err == nil {
				err = e
			}
		}
		done <- err
	}
	go copyOne(a, b)
	go copyOne(b, a)
	first := <-done
	if first != nil {
		a.Close()
		b.Close()
	}
	return errors.Join(first, <-done)
}

func writeNewFile(path string, data []byte) error {
	f, err := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	_, err = f.Write(data)
	closeErr := f.Close()
	return errors.Join(err, closeErr)
}

// Rendezvous readers must never observe an empty or partially written file. The
// device lock and private directory give this publisher exclusive ownership of
// these paths; PowerShell only writes its separate authorized.json receipt.
func publishFile(path string, data []byte) error {
	if _, err := os.Lstat(path); !errors.Is(err, os.ErrNotExist) {
		return fmt.Errorf("rendezvous path already exists or is unreadable: %s", path)
	}
	f, err := os.CreateTemp(filepath.Dir(path), ".publish-")
	if err != nil {
		return err
	}
	name := f.Name()
	defer os.Remove(name)
	if _, err = f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err = f.Close(); err != nil {
		return err
	}
	return os.Rename(name, path)
}
