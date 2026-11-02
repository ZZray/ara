// Copyright 2026 Ray Zhang.
package main

import (
	"bytes"
	"context"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"golang.org/x/crypto/ssh"
)

func testSigner(t *testing.T) ssh.Signer {
	t.Helper()
	s, _, err := newSigner()
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func testHub(t *testing.T) (*hub, *httptest.Server) {
	t.Helper()
	h, err := newHub()
	if err != nil {
		t.Fatal(err)
	}
	s := httptest.NewTLSServer(h)
	t.Cleanup(func() { h.Close(); s.Close() })
	return h, s
}

type deviceRun struct {
	Pair   pairing
	Done   <-chan error
	Cancel context.CancelFunc
}

func testDevice(t *testing.T, server string, hc *http.Client, name string, auth authorizeFunc) deviceRun {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	pairs := make(chan pairing, 1)
	done := make(chan error, 1)
	go func() {
		done <- runDevice(ctx, server, name, testSigner(t), auth, func(p pairing) { pairs <- p }, hc, false)
	}()
	var p pairing
	select {
	case p = <-pairs:
	case err := <-done:
		cancel()
		t.Fatalf("register: %v", err)
	case <-time.After(5 * time.Second):
		cancel()
		t.Fatal("register timed out")
	}
	t.Cleanup(func() {
		cancel()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Error("controlled session did not exit")
		}
	})
	return deviceRun{Pair: p, Done: done, Cancel: cancel}
}

func testController(t *testing.T, server string, hc *http.Client) (*controller, ssh.Signer) {
	t.Helper()
	signer := testSigner(t)
	c, err := newController(context.Background(), server, signer, hc, false)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		c.transport.Close()
		for _, d := range c.devices {
			d.Close()
		}
	})
	return c, signer
}

func testTargets(t *testing.T, address string) map[string]target {
	t.Helper()
	return map[string]target{"jump": {Address: address, User: "fixture", HostKey: string(ssh.MarshalAuthorizedKey(testSigner(t).PublicKey()))}}
}

func waitFor(t *testing.T, condition func() bool) {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if condition() {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("condition did not become true")
}

func TestPairingAtomicSingleUseAndOwnerIsolation(t *testing.T) {
	h, server := testHub(t)
	targets := testTargets(t, "127.0.0.1:1")
	seenKey := make(chan ssh.PublicKey, 1)
	d := testDevice(t, server.URL, server.Client(), "one", func(ctx context.Context, key ssh.PublicKey) (authorization, error) {
		seenKey <- key
		return authorization{OK: true, Targets: targets}, nil
	})
	// A request already queued when its transport was removed must not consume
	// the still-valid code or start local key authorization.
	if _, err := h.pair(&peer{key: testSigner(t).PublicKey()}, encode(struct {
		Code string `json:"code"`
	}{d.Pair.Code})); err == nil {
		t.Fatal("disconnected controller claimed pairing")
	}
	c1, k1 := testController(t, server.URL, server.Client())
	c2, k2 := testController(t, server.URL, server.Client())
	type result struct {
		c    *controller
		k    ssh.Signer
		info deviceInfo
		err  error
	}
	results := make(chan result, 2)
	for _, v := range []struct {
		c *controller
		k ssh.Signer
	}{{c1, k1}, {c2, k2}} {
		go func() { info, err := v.c.pair(d.Pair.Code); results <- result{v.c, v.k, info, err} }()
	}
	a, b := <-results, <-results
	if (a.err == nil) == (b.err == nil) {
		t.Fatalf("wanted exactly one pair winner; success=%v/%v", a.err == nil, b.err == nil)
	}
	winner, loser := a, b
	if winner.err != nil {
		winner, loser = loser, winner
	}
	if !bytes.Equal((<-seenKey).Marshal(), winner.k.PublicKey().Marshal()) {
		t.Fatal("authorization key did not come from winning SSH identity")
	}
	if _, err := winner.c.pair(d.Pair.Code); err == nil {
		t.Fatal("pair code was reusable")
	}
	if err := loser.c.stop(d.Pair.DeviceID); err == nil {
		t.Fatal("non-owner stopped device")
	}
	if _, err := loser.c.open(d.Pair.DeviceID, "jump"); err == nil {
		t.Fatal("non-owner opened device")
	}
	response, err := request(loser.c.transport.Conn, "ry-list", nil, time.Second)
	if err != nil || string(response) != "[]" {
		t.Fatalf("non-owner saw devices: %s %v", response, err)
	}
	if err = winner.c.stop(d.Pair.DeviceID); err != nil {
		t.Fatal(err)
	}
	// Device exit is verified by its cleanup waiter; the unrelated controller remains alive.
	if _, err = request(loser.c.transport.Conn, "ry-list", nil, time.Second); err != nil {
		t.Fatal(err)
	}
}

func TestGeneratedKeyAcceptedByOpenSSH(t *testing.T) {
	binary, err := exec.LookPath("ssh-keygen")
	if err != nil {
		t.Skip("OpenSSH key parser is unavailable")
	}
	dir := t.TempDir()
	if err = secureDir(dir); err != nil {
		t.Fatal(err)
	}
	signer, key, err := newSigner()
	if err != nil {
		t.Fatal(err)
	}
	block, err := ssh.MarshalPrivateKey(key, "RY fixture key")
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "fixture.key")
	if err = writeNewFile(path, pem.EncodeToMemory(block)); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	output, err := exec.CommandContext(ctx, binary, "-y", "-f", path).CombinedOutput()
	if err != nil {
		t.Fatalf("OpenSSH could not read protected temporary key: %v %s", err, output)
	}
	parsed, err := publicKey(string(output))
	if err != nil || !bytes.Equal(parsed.Marshal(), signer.PublicKey().Marshal()) {
		t.Fatal("OpenSSH public key did not match generated private key")
	}
}

func TestExpiredCodeAndInvalidAuthorization(t *testing.T) {
	t.Run("expired", func(t *testing.T) {
		h, server := testHub(t)
		h.pairTTL = 50 * time.Millisecond
		d := testDevice(t, server.URL, server.Client(), "expired", func(context.Context, ssh.PublicKey) (authorization, error) {
			t.Error("expired authorization was invoked")
			return authorization{}, nil
		})
		time.Sleep(100 * time.Millisecond)
		c, _ := testController(t, server.URL, server.Client())
		if _, err := c.pair(d.Pair.Code); err == nil {
			t.Fatal("expired code accepted")
		}
	})
	t.Run("invalid-receipt", func(t *testing.T) {
		_, server := testHub(t)
		d := testDevice(t, server.URL, server.Client(), "invalid", func(context.Context, ssh.PublicKey) (authorization, error) {
			return authorization{OK: true, Targets: map[string]target{"arbitrary": {Address: "127.0.0.1:22", User: "u", HostKey: "bad"}}}, nil
		})
		c, _ := testController(t, server.URL, server.Client())
		if _, err := c.pair(d.Pair.Code); err == nil {
			t.Fatal("bad receipt reported ready")
		}
		if _, err := c.open(d.Pair.DeviceID, "arbitrary"); err == nil {
			t.Fatal("bad target was opened")
		}
	})
}

func TestControllerDisconnectCancelsPendingAuthorization(t *testing.T) {
	h, server := testHub(t)
	entered := make(chan struct{})
	cancelled := make(chan struct{})
	d := testDevice(t, server.URL, server.Client(), "pending", func(ctx context.Context, _ ssh.PublicKey) (authorization, error) {
		close(entered)
		<-ctx.Done()
		close(cancelled)
		return authorization{}, ctx.Err()
	})
	c, _ := testController(t, server.URL, server.Client())
	pairDone := make(chan error, 1)
	go func() { _, err := c.pair(d.Pair.Code); pairDone <- err }()
	select {
	case <-entered:
	case <-time.After(3 * time.Second):
		t.Fatal("authorizer not called")
	}
	c.transport.Close()
	select {
	case <-cancelled:
	case <-time.After(3 * time.Second):
		t.Fatal("disconnect left authorizer running")
	}
	select {
	case err := <-pairDone:
		if err == nil {
			t.Fatal("disconnected pairing succeeded")
		}
	case <-time.After(3 * time.Second):
		t.Fatal("pair request leaked")
	}
	waitFor(t, func() bool { h.mu.Lock(); defer h.mu.Unlock(); return len(h.devices) == 0 })
}

func TestMultipleDevicesHalfCloseAndStopIsolation(t *testing.T) {
	_, server := testHub(t)
	backend, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer backend.Close()
	var backendWorkers sync.WaitGroup
	backendWorkers.Go(func() {
		for {
			conn, err := backend.Accept()
			if err != nil {
				return
			}
			backendWorkers.Go(func() {
				defer conn.Close()
				conn.SetDeadline(time.Now().Add(4 * time.Second))
				data, _ := io.ReadAll(conn)
				conn.Write(append([]byte("response:"), data...))
				conn.(*net.TCPConn).CloseWrite()
			})
		}
	})
	t.Cleanup(func() { backend.Close(); backendWorkers.Wait() })
	targets := testTargets(t, backend.Addr().String())
	targets["backend"] = targets["jump"]
	auth := func(context.Context, ssh.PublicKey) (authorization, error) {
		return authorization{OK: true, Targets: targets}, nil
	}
	d1 := testDevice(t, server.URL, server.Client(), "first", auth)
	d2 := testDevice(t, server.URL, server.Client(), "second", auth)
	c, _ := testController(t, server.URL, server.Client())
	i1, err := c.pair(d1.Pair.Code)
	if err != nil {
		t.Fatal(err)
	}
	i2, err := c.pair(d2.Pair.Code)
	if err != nil {
		t.Fatal(err)
	}
	probe := func(id, name string) {
		ch, err := c.open(id, name)
		if err != nil {
			t.Fatal(err)
		}
		defer ch.Close()
		ch.Write([]byte("hello"))
		ch.CloseWrite()
		data, err := io.ReadAll(ch)
		if err != nil || string(data) != "response:hello" {
			t.Fatalf("half-close response: %q %v", data, err)
		}
	}
	probe(i1.DeviceID, "jump")
	probe(i1.DeviceID, "backend")
	probe(i2.DeviceID, "jump")
	if _, err = c.open(i2.DeviceID, "unconfigured"); err == nil {
		t.Fatal("unknown target accepted")
	}
	active, err := c.open(i1.DeviceID, "jump")
	if err != nil {
		t.Fatal(err)
	}
	if err = c.stop(i1.DeviceID); err != nil {
		t.Fatal(err)
	}
	closed := make(chan struct{})
	go func() { io.Copy(io.Discard, active); close(closed) }()
	select {
	case <-closed:
	case <-time.After(3 * time.Second):
		t.Fatal("stop did not cancel active data stream")
	}
	probe(i2.DeviceID, "backend")
	if _, err = c.open(i1.DeviceID, "jump"); err == nil {
		t.Fatal("stopped device reopened")
	}
}

func TestUnavailableTarget(t *testing.T) {
	_, server := testHub(t)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	address := listener.Addr().String()
	listener.Close()
	d := testDevice(t, server.URL, server.Client(), "unavailable", func(context.Context, ssh.PublicKey) (authorization, error) {
		return authorization{OK: true, Targets: testTargets(t, address)}, nil
	})
	c, _ := testController(t, server.URL, server.Client())
	info, err := c.pair(d.Pair.Code)
	if err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	if _, err = c.open(info.DeviceID, "jump"); err == nil {
		t.Fatal("unavailable target succeeded")
	}
	if time.Since(start) > 6*time.Second {
		t.Fatal("target failure was not bounded")
	}
	if _, err = request(c.transport.Conn, "ry-list", nil, time.Second); err != nil {
		t.Fatal("target error killed controller")
	}
}

func TestFileAuthorizationAndInputValidation(t *testing.T) {
	dir := t.TempDir()
	key := testSigner(t).PublicKey()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	result := make(chan error, 1)
	go func() { _, err := fileAuthorizer(dir)(ctx, key); result <- err }()
	waitFor(t, func() bool { _, err := os.Stat(filepath.Join(dir, "controller.pub")); return err == nil })
	data, err := os.ReadFile(filepath.Join(dir, "controller.pub"))
	if err != nil || !bytes.Equal(data, ssh.MarshalAuthorizedKey(key)) {
		t.Fatal("wrong public key rendezvous")
	}
	receipt := authorization{OK: true, Targets: testTargets(t, "127.0.0.1:22")}
	if err = os.WriteFile(filepath.Join(dir, "authorized.json"), append([]byte{0xef, 0xbb, 0xbf}, encode(receipt)...), 0600); err != nil {
		t.Fatal(err)
	}
	select {
	case err = <-result:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(time.Second):
		t.Fatal("receipt not consumed")
	}
	if _, err = fileAuthorizer(dir)(ctx, key); err == nil {
		t.Fatal("existing controller public key was overwritten")
	}
	for _, raw := range []string{"http://example.com", "https://user:pass@example.com", "https://example.com?secret=x", "https://example.com/#x", "file:///tmp/a"} {
		if _, err = serverURL(raw, false); err == nil {
			t.Fatalf("unsafe URL accepted: %s", raw)
		}
	}
	if _, err = publicKey("command=\"anything\" " + string(data)); err == nil {
		t.Fatal("authorized key options accepted")
	}
	if _, err = publicKey(string(data) + string(data)); err == nil {
		t.Fatal("multiple public keys accepted")
	}
}

func TestRelayRejectsBrowserAndCapsConnections(t *testing.T) {
	h, server := testHub(t)
	req, _ := http.NewRequest(http.MethodGet, server.URL+"/connect", nil)
	req.Header.Set("Origin", "https://example.com")
	response, err := server.Client().Do(req)
	if err != nil {
		t.Fatal(err)
	}
	response.Body.Close()
	if response.StatusCode != 403 {
		t.Fatalf("browser status %d", response.StatusCode)
	}
	for i := 0; i < cap(h.slots); i++ {
		h.slots <- struct{}{}
	}
	response, err = server.Client().Get(server.URL + "/connect")
	if err != nil {
		t.Fatal(err)
	}
	response.Body.Close()
	if response.StatusCode != 503 {
		t.Fatalf("capacity status %d", response.StatusCode)
	}
	for i := 0; i < cap(h.slots); i++ {
		<-h.slots
	}
}

func TestStreamLimitAndLocalEndpoint(t *testing.T) {
	_, server := testHub(t)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	var workers sync.WaitGroup
	workers.Go(func() {
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			workers.Go(func() {
				defer conn.Close()
				stop := context.AfterFunc(ctx, func() { conn.Close() })
				defer stop()
				io.Copy(conn, conn)
			})
		}
	})
	t.Cleanup(func() { cancel(); listener.Close(); workers.Wait() })
	targets := testTargets(t, listener.Addr().String())
	d := testDevice(t, server.URL, server.Client(), "limited", func(context.Context, ssh.PublicKey) (authorization, error) {
		return authorization{OK: true, Targets: targets}, nil
	})
	c, _ := testController(t, server.URL, server.Client())
	info, err := c.pair(d.Pair.Code)
	if err != nil {
		t.Fatal(err)
	}
	streams := make([]ssh.Channel, 0, maxStreams)
	defer func() {
		for _, ch := range streams {
			ch.Close()
		}
	}()
	for range maxStreams {
		ch, err := c.open(info.DeviceID, "jump")
		if err != nil {
			t.Fatal(err)
		}
		streams = append(streams, ch)
	}
	if ch, err := c.open(info.DeviceID, "jump"); err == nil {
		ch.Close()
		t.Fatal("seventeenth stream accepted")
	}
	for _, ch := range streams {
		ch.Close()
	}
	streams = nil
	// Exercise the exact listener path handed to external ssh/scp clients.
	dir := t.TempDir()
	c.hostsPath = filepath.Join(dir, "known_hosts")
	c.keyPath = filepath.Join(dir, "key")
	if err = os.WriteFile(c.hostsPath, nil, 0600); err != nil {
		t.Fatal(err)
	}
	n, err := c.expose(info)
	if err != nil {
		t.Fatal(err)
	}
	endpoint := c.devices[n].endpoints["jump"]
	local, err := net.DialTimeout("tcp", endpoint.listener.Addr().String(), time.Second)
	if err != nil {
		t.Fatal(err)
	}
	defer local.Close()
	local.SetDeadline(time.Now().Add(3 * time.Second))
	local.Write([]byte("through-local-port"))
	data := make([]byte, len("through-local-port"))
	if _, err = io.ReadFull(local, data); err != nil || string(data) != "through-local-port" {
		t.Fatalf("local forwarding: %q %v", data, err)
	}
	known, err := os.ReadFile(c.hostsPath)
	if err != nil || !strings.Contains(string(known), endpoint.alias+" ssh-ed25519 ") {
		t.Fatalf("host pin file: %v", err)
	}
	if !strings.Contains(strings.Join(c.sshArgs(endpoint), " "), "StrictHostKeyChecking=yes") {
		t.Fatal("external SSH command does not enforce host pin")
	}
	if err = c.stop(info.DeviceID); err != nil {
		t.Fatal(err)
	}
	c.devices[n].Close()
	delete(c.devices, n)
	if conn, err := net.DialTimeout("tcp", endpoint.listener.Addr().String(), time.Second); err == nil {
		conn.Close()
		t.Fatal("local port still listens after close")
	}
}

func TestTransportHandshakeCancellationAndHostPin(t *testing.T) {
	t.Run("stalled-handshake", func(t *testing.T) {
		host := testSigner(t)
		server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.URL.Path == "/host-key" {
				w.Write(ssh.MarshalAuthorizedKey(host.PublicKey()))
				return
			}
			ws, err := websocket.Accept(w, r, nil)
			if err != nil {
				return
			}
			defer ws.CloseNow()
			conn := websocket.NetConn(r.Context(), ws, websocket.MessageBinary)
			defer conn.Close()
			io.Copy(io.Discard, conn)
		}))
		defer server.Close()
		ctx, cancel := context.WithTimeout(context.Background(), 150*time.Millisecond)
		defer cancel()
		start := time.Now()
		transport, err := dialTransport(ctx, server.URL, "controller", testSigner(t), server.Client(), false)
		if err == nil {
			transport.Close()
			t.Fatal("stalled handshake succeeded")
		}
		if time.Since(start) > 2*time.Second {
			t.Fatal("context did not bound handshake")
		}
	})
	t.Run("relay-host-pin", func(t *testing.T) {
		h, err := newHub()
		if err != nil {
			t.Fatal(err)
		}
		defer h.Close()
		wrong := testSigner(t)
		server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.URL.Path == "/host-key" {
				w.Write(ssh.MarshalAuthorizedKey(wrong.PublicKey()))
				return
			}
			h.ServeHTTP(w, r)
		}))
		defer server.Close()
		tr, err := dialTransport(context.Background(), server.URL, "controller", testSigner(t), server.Client(), false)
		if err == nil {
			tr.Close()
			t.Fatal("wrong relay host key accepted")
		}
	})
}

// This fixture speaks actual SSH, authenticates the paired public key, and only
// executes the single read-only "probe" command. It is never an instrument.
type sshFixture struct {
	listener net.Listener
	host     ssh.Signer
	mu       sync.Mutex
	allowed  ssh.PublicKey
	workers  sync.WaitGroup
	ctx      context.Context
	cancel   context.CancelFunc
}

func newSSHFixture(t *testing.T) *sshFixture {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	f := &sshFixture{listener: l, host: testSigner(t), ctx: ctx, cancel: cancel}
	f.workers.Go(func() {
		for {
			raw, err := l.Accept()
			if err != nil {
				return
			}
			f.workers.Go(func() {
				defer raw.Close()
				stop := context.AfterFunc(ctx, func() { raw.Close() })
				defer stop()
				raw.SetDeadline(time.Now().Add(5 * time.Second))
				config := &ssh.ServerConfig{PublicKeyCallback: func(_ ssh.ConnMetadata, key ssh.PublicKey) (*ssh.Permissions, error) {
					f.mu.Lock()
					defer f.mu.Unlock()
					if f.allowed != nil && bytes.Equal(f.allowed.Marshal(), key.Marshal()) {
						return nil, nil
					}
					return nil, errors.New("unpaired key")
				}}
				config.AddHostKey(f.host)
				conn, channels, requests, err := ssh.NewServerConn(raw, config)
				if err != nil {
					return
				}
				defer conn.Close()
				raw.SetDeadline(time.Time{})
				go ssh.DiscardRequests(requests)
				for ch := range channels {
					if ch.ChannelType() != "session" {
						ch.Reject(ssh.Prohibited, "fixture only")
						continue
					}
					stream, requests, err := ch.Accept()
					if err != nil {
						return
					}
					f.workers.Go(func() {
						defer stream.Close()
						for r := range requests {
							var command struct{ Command string }
							valid := r.Type == "exec" && ssh.Unmarshal(r.Payload, &command) == nil && command.Command == "probe"
							r.Reply(valid, nil)
							if valid {
								io.WriteString(stream, "RY fixture probe OK\n")
								stream.SendRequest("exit-status", false, ssh.Marshal(struct{ Status uint32 }{0}))
								stream.CloseWrite()
								return
							}
						}
					})
				}
			})
		}
	})
	t.Cleanup(func() { cancel(); l.Close(); f.workers.Wait() })
	return f
}

func (f *sshFixture) authorize(_ context.Context, key ssh.PublicKey) (authorization, error) {
	f.mu.Lock()
	f.allowed = key
	f.mu.Unlock()
	t := target{Address: f.listener.Addr().String(), User: "fixture", HostKey: string(ssh.MarshalAuthorizedKey(f.host.PublicKey()))}
	return authorization{OK: true, Targets: map[string]target{"jump": t, "backend": t}}, nil
}

type channelConn struct{ ssh.Channel }

func (c channelConn) LocalAddr() net.Addr              { return &net.TCPAddr{IP: net.IPv4(127, 0, 0, 1)} }
func (c channelConn) RemoteAddr() net.Addr             { return &net.TCPAddr{IP: net.IPv4(127, 0, 0, 1)} }
func (c channelConn) SetDeadline(time.Time) error      { return nil }
func (c channelConn) SetReadDeadline(time.Time) error  { return nil }
func (c channelConn) SetWriteDeadline(time.Time) error { return nil }

func probeFixture(t *testing.T, c *controller, signer ssh.Signer, info deviceInfo, name string, host ssh.PublicKey) error {
	t.Helper()
	ch, err := c.open(info.DeviceID, name)
	if err != nil {
		return err
	}
	defer ch.Close()
	timeout := time.AfterFunc(8*time.Second, func() { ch.Close() })
	defer timeout.Stop()
	cc, channels, requests, err := ssh.NewClientConn(channelConn{ch}, "fixture", &ssh.ClientConfig{User: "fixture", Auth: []ssh.AuthMethod{ssh.PublicKeys(signer)}, HostKeyCallback: ssh.FixedHostKey(host)})
	if err != nil {
		return err
	}
	client := ssh.NewClient(cc, channels, requests)
	defer client.Close()
	session, err := client.NewSession()
	if err != nil {
		return err
	}
	defer session.Close()
	output, err := session.Output("probe")
	if err != nil {
		return err
	}
	if string(output) != "RY fixture probe OK\n" {
		return fmt.Errorf("wrong fixture result: %q", output)
	}
	return nil
}

func runEndToEnd(t *testing.T, server string, hc *http.Client) {
	f := newSSHFixture(t)
	d := testDevice(t, server, hc, "RY-controlled-fixture", f.authorize)
	c, key := testController(t, server, hc)
	info, err := c.pair(d.Pair.Code)
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"jump", "backend"} {
		if err = probeFixture(t, c, key, info, name, f.host.PublicKey()); err != nil {
			t.Fatalf("%s SSH: %v", name, err)
		}
	}
	if err = probeFixture(t, c, key, info, "jump", testSigner(t).PublicKey()); err == nil {
		t.Fatal("wrong inner host key accepted")
	}
	if err = c.stop(info.DeviceID); err != nil {
		t.Fatal(err)
	}
	if _, err = c.open(info.DeviceID, "jump"); err == nil {
		t.Fatal("stopped fixture remained reachable")
	}
	t.Log("actual TLS/WebSocket/SSH pairing, jump+backend login, host-key rejection and stop verified")
}

func TestEndToEndSSH(t *testing.T) {
	_, server := testHub(t)
	runEndToEnd(t, server.URL, server.Client())
}

func TestExternalSSHWithSpacedStateDirectory(t *testing.T) {
	binary, err := exec.LookPath("ssh")
	if err != nil {
		t.Skip("OpenSSH client unavailable")
	}
	_, server := testHub(t)
	f := newSSHFixture(t)
	d := testDevice(t, server.URL, server.Client(), "native-ssh", f.authorize)
	signer, private, err := newSigner()
	if err != nil {
		t.Fatal(err)
	}
	c, err := newController(context.Background(), server.URL, signer, server.Client(), false)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		c.transport.Close()
		for _, d := range c.devices {
			d.Close()
		}
	})
	info, err := c.pair(d.Pair.Code)
	if err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(t.TempDir(), "support files 100% ready")
	if err = os.Mkdir(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err = secureDir(dir); err != nil {
		t.Fatal(err)
	}
	c.keyPath = filepath.Join(dir, "temporary key")
	c.hostsPath = filepath.Join(dir, "known hosts")
	block, err := ssh.MarshalPrivateKey(private, "RY native SSH test")
	if err != nil {
		t.Fatal(err)
	}
	if err = writeNewFile(c.keyPath, pem.EncodeToMemory(block)); err != nil {
		t.Fatal(err)
	}
	if err = writeNewFile(c.hostsPath, nil); err != nil {
		t.Fatal(err)
	}
	n, err := c.expose(info)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	args := append(c.sshArgs(c.devices[n].endpoints["jump"]), "probe")
	output, err := exec.CommandContext(ctx, binary, args...).CombinedOutput()
	if err != nil || !strings.Contains(string(output), "RY fixture probe OK") {
		t.Fatalf("external SSH through quoted known-hosts path: %v %s", err, output)
	}
	wrong := testSigner(t).PublicKey()
	alias := c.devices[n].endpoints["jump"].alias
	if err = os.WriteFile(c.hostsPath, []byte(alias+" "+string(ssh.MarshalAuthorizedKey(wrong))), 0600); err != nil {
		t.Fatal(err)
	}
	output, err = exec.CommandContext(ctx, binary, args...).CombinedOutput()
	if err == nil {
		t.Fatalf("external SSH accepted changed host key: %s", output)
	}
}

func TestLiveRelay(t *testing.T) {
	server := os.Getenv("RY_SSH_TUNNEL_LIVE_URL")
	if server == "" {
		t.Skip("set RY_SSH_TUNNEL_LIVE_URL to test a deployed relay with a local controlled SSH fixture")
	}
	if !strings.HasPrefix(server, "https://") {
		t.Fatal("live relay URL must use HTTPS")
	}
	runEndToEnd(t, server, nil)
}
