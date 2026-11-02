// Copyright 2026 Ray Zhang.
package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
)

func directTestDevice(t *testing.T, server string, hc *http.Client, cfg directConfig, targets map[string]target) pairing {
	t.Helper()
	ctx, cancel := context.WithCancel(context.WithValue(context.Background(), directConfigKey{}, cfg))
	signer := testSigner(t)
	pairs := make(chan pairing, 1)
	done := make(chan error, 1)
	go func() {
		done <- runDevice(ctx, server, "direct fixture", signer, func(context.Context, ssh.PublicKey) (authorization, error) {
			return authorization{OK: true, Targets: targets}, nil
		}, func(p pairing) { pairs <- p }, hc, false)
	}()
	t.Cleanup(func() {
		cancel()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Error("direct device did not shut down")
		}
	})
	select {
	case p := <-pairs:
		return p
	case err := <-done:
		t.Fatalf("register: %v", err)
	case <-time.After(5 * time.Second):
		t.Fatal("direct registration timeout")
	}
	return pairing{}
}

func directTestController(t *testing.T, server string, hc *http.Client) *controller {
	t.Helper()
	ctx := context.WithValue(context.Background(), directConfigKey{}, directConfig{loopback: true})
	c, err := newController(ctx, server, testSigner(t), hc, false)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { c.transport.Close() })
	return c
}

func directTestTarget(t *testing.T) (map[string]target, *atomic.Int32) {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	var workers sync.WaitGroup
	accepted := new(atomic.Int32)
	workers.Go(func() {
		for {
			conn, err := l.Accept()
			if err != nil {
				return
			}
			accepted.Add(1)
			workers.Go(func() {
				defer conn.Close()
				stop := context.AfterFunc(ctx, func() { conn.Close() })
				defer stop()
				conn.SetDeadline(time.Now().Add(12 * time.Second))
				data, err := io.ReadAll(conn)
				if err == nil {
					conn.Write(data)
					conn.(*net.TCPConn).CloseWrite()
				}
			})
		}
	})
	t.Cleanup(func() { cancel(); l.Close(); workers.Wait() })
	return testTargets(t, l.Addr().String()), accepted
}

func directRoundTrip(c *controller, id string, payload []byte) error {
	ch, err := c.open(id, "jump")
	if err != nil {
		return err
	}
	defer ch.Close()
	timer := time.AfterFunc(8*time.Second, func() { ch.Close() })
	defer timer.Stop()
	if _, err = ch.Write(payload); err != nil {
		return err
	}
	if err = ch.CloseWrite(); err != nil {
		return err
	}
	data, err := io.ReadAll(ch)
	if err != nil {
		return err
	}
	if !bytes.Equal(data, payload) {
		return fmt.Errorf("payload mismatch: got=%d want=%d", len(data), len(payload))
	}
	return nil
}

func directTestPair(t *testing.T, c *controller, p pairing) deviceInfo {
	t.Helper()
	i, err := c.pair(p.Code)
	if err != nil {
		t.Fatal(err)
	}
	if err = c.tryDirect(i); err != nil {
		t.Fatal(err)
	}
	return i
}

func TestDirectDataConnSmallReads(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.WithValue(context.Background(), directConfigKey{}, directConfig{loopback: true}), 10*time.Second)
	defer cancel()
	a, err := newDirectPeer(ctx, true)
	if err != nil {
		t.Fatal(err)
	}
	defer a.Close()
	b, err := newDirectPeer(ctx, false)
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	offer, err := a.description(ctx, nil)
	if err != nil {
		t.Fatal(err)
	}
	answer, err := b.description(ctx, &offer)
	if err != nil {
		t.Fatal(err)
	}
	if err = a.pc.SetRemoteDescription(answer); err != nil {
		t.Fatal(err)
	}
	var ac, bc *dataConn
	select {
	case ac = <-a.opened:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	select {
	case bc = <-b.opened:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	deadline, _ := ctx.Deadline()
	ac.SetDeadline(deadline)
	bc.SetDeadline(deadline)
	payload := bytes.Repeat([]byte("small-read-boundary"), 8192)
	written := make(chan error, 1)
	go func() { _, err := ac.Write(payload); written <- err }()
	got := make([]byte, len(payload))
	for offset := 0; offset < len(got); {
		n, err := bc.Read(got[offset:min(offset+7, len(got))])
		if err != nil {
			t.Fatal(err)
		}
		offset += n
	}
	if err = <-written; err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, payload) {
		t.Fatal("small reads lost message bytes")
	}
}

func TestDirectConcurrentPayloadBypassesRelay(t *testing.T) {
	h, server := testHub(t)
	targets, accepted := directTestTarget(t)
	p := directTestDevice(t, server.URL, server.Client(), directConfig{loopback: true}, targets)
	c := directTestController(t, server.URL, server.Client())
	i := directTestPair(t, c, p)
	h.mu.Lock()
	d := h.devices[i.DeviceID]
	h.mu.Unlock()
	// Saturating the hub's relay slots makes every ry-forward fail. Successful
	// payloads must therefore use the separately authenticated peer connection.
	for range cap(d.streams) {
		d.streams <- struct{}{}
	}
	defer func() {
		for range cap(d.streams) {
			<-d.streams
		}
	}()
	blocked, _, err := c.transport.Conn.OpenChannel("ry-forward", encode(openRequest{DeviceID: i.DeviceID, Target: "jump"}))
	if err == nil {
		blocked.Close()
		t.Fatal("relay guard did not reject forwarding")
	}
	var workers sync.WaitGroup
	for n := range 4 {
		workers.Go(func() {
			payload := bytes.Repeat([]byte{byte(n), 0, 255, 42}, 256*1024)
			if err := directRoundTrip(c, i.DeviceID, payload); err != nil {
				t.Error(err)
			}
		})
	}
	workers.Wait()
	if accepted.Load() != 4 {
		t.Fatalf("target opens=%d, want 4", accepted.Load())
	}
	t.Log("4 concurrent 1 MiB request/response streams succeeded with hub relay slots blocked")
}

func TestDirectStopIsolationAndSharedLimit(t *testing.T) {
	_, server := testHub(t)
	targets, _ := directTestTarget(t)
	p1 := directTestDevice(t, server.URL, server.Client(), directConfig{loopback: true}, targets)
	p2 := directTestDevice(t, server.URL, server.Client(), directConfig{loopback: true}, targets)
	c := directTestController(t, server.URL, server.Client())
	i1, i2 := directTestPair(t, c, p1), directTestPair(t, c, p2)
	channels := make([]ssh.Channel, 0, maxStreams)
	defer func() {
		for _, ch := range channels {
			ch.Close()
		}
	}()
	for range maxStreams {
		ch, err := c.open(i1.DeviceID, "jump")
		if err != nil {
			t.Fatal(err)
		}
		channels = append(channels, ch)
	}
	if ch, err := c.open(i1.DeviceID, "jump"); err == nil {
		ch.Close()
		t.Fatal("direct stream limit not enforced")
	}
	if ch, _, err := c.transport.Conn.OpenChannel("ry-forward", encode(openRequest{DeviceID: i1.DeviceID, Target: "jump"})); err == nil {
		ch.Close()
		t.Fatal("relay bypassed shared device stream limit")
	}
	if err := c.stop(i1.DeviceID); err != nil {
		t.Fatal(err)
	}
	closed := make(chan struct{})
	go func() { io.Copy(io.Discard, channels[0]); close(closed) }()
	select {
	case <-closed:
	case <-time.After(3 * time.Second):
		t.Fatal("stop left a direct stream open")
	}
	if err := directRoundTrip(c, i2.DeviceID, []byte("other device remains live")); err != nil {
		t.Fatal(err)
	}
	if ch, err := c.open(i1.DeviceID, "jump"); err == nil {
		ch.Close()
		t.Fatal("stopped device reopened")
	}
}

func TestDirectFailureDoesNotReplayAndNewStreamFallsBack(t *testing.T) {
	_, server := testHub(t)
	targets, accepted := directTestTarget(t)
	p := directTestDevice(t, server.URL, server.Client(), directConfig{loopback: true}, targets)
	c := directTestController(t, server.URL, server.Client())
	i := directTestPair(t, c, p)
	ch, err := c.open(i.DeviceID, "jump")
	if err != nil {
		t.Fatal(err)
	}
	defer ch.Close()
	if _, err = ch.Write([]byte("started operation")); err != nil {
		t.Fatal(err)
	}
	value, _ := c.direct.Load(i.DeviceID)
	s := value.(*directSession)
	s.peer.Close()
	select {
	case <-s.done:
	case <-time.After(3 * time.Second):
		t.Fatal("direct SSH did not close")
	}
	if _, err = ch.Write([]byte("must not replay")); err == nil {
		t.Fatal("closed old stream accepted data")
	}
	if accepted.Load() != 1 {
		t.Fatalf("old stream replayed: target opens=%d", accepted.Load())
	}
	if err = directRoundTrip(c, i.DeviceID, []byte("new relay operation")); err != nil {
		t.Fatal(err)
	}
	if accepted.Load() != 2 {
		t.Fatalf("new stream opens=%d, want 2 total", accepted.Load())
	}
}

func TestDirectAuthorizationAndDisabledFallback(t *testing.T) {
	for _, mode := range []string{"wrong-key", "wrong-host", "disabled"} {
		t.Run(mode, func(t *testing.T) {
			h, server := testHub(t)
			targets, _ := directTestTarget(t)
			p := directTestDevice(t, server.URL, server.Client(), directConfig{loopback: true, disabled: mode == "disabled"}, targets)
			c := directTestController(t, server.URL, server.Client())
			i, err := c.pair(p.Code)
			if err != nil {
				t.Fatal(err)
			}
			other := directTestController(t, server.URL, server.Client())
			if ch, _, err := other.transport.Conn.OpenChannel(directProtocol, encode(openRequest{DeviceID: i.DeviceID})); err == nil {
				ch.Close()
				t.Fatal("non-owner obtained signaling channel")
			}
			original := c.signer
			if mode == "wrong-key" {
				c.signer = testSigner(t)
			}
			if mode == "wrong-host" {
				// Corrupt the hub's pinned identity, while the device still proves
				// possession of its actual private key on the peer connection.
				h.mu.Lock()
				h.devices[i.DeviceID].peer.key = testSigner(t).PublicKey()
				h.mu.Unlock()
			}
			err = c.tryDirect(i)
			c.signer = original
			if err == nil {
				t.Fatal("unauthorized/disabled direct connection accepted")
			}
			if err = directRoundTrip(c, i.DeviceID, []byte("relay survives failed direct setup")); err != nil {
				t.Fatal(err)
			}
			if err = c.stop(i.DeviceID); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestDirectUnresponsiveSTUNStillUsesHostCandidates(t *testing.T) {
	blackhole, err := net.ListenPacket("udp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer blackhole.Close()
	cfg := directConfig{loopback: true, stun: []string{"stun:" + blackhole.LocalAddr().String()}}
	_, server := testHub(t)
	targets, _ := directTestTarget(t)
	p := directTestDevice(t, server.URL, server.Client(), cfg, targets)
	c := directTestController(t, server.URL, server.Client())
	c.ctx = context.WithValue(c.ctx, directConfigKey{}, cfg)
	started := time.Now()
	i := directTestPair(t, c, p)
	if elapsed := time.Since(started); elapsed > 8*time.Second {
		t.Fatalf("STUN gathering was not bounded: %s", elapsed)
	}
	if err = directRoundTrip(c, i.DeviceID, []byte("STUN unavailable; host path remains usable")); err != nil {
		t.Fatal(err)
	}
}

func TestDirectSignalingTimeoutPreservesRelayAndStop(t *testing.T) {
	blackhole, err := net.ListenPacket("udp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer blackhole.Close()
	_, server := testHub(t)
	targets, _ := directTestTarget(t)
	// The device's actual STUN gathering delays its SDP answer. Give only this
	// optional negotiation a short deadline; the relay transport retains its
	// original lifetime context, exactly as a timed-out optional setup should.
	p := directTestDevice(t, server.URL, server.Client(), directConfig{loopback: true, stun: []string{"stun:" + blackhole.LocalAddr().String()}}, targets)
	c := directTestController(t, server.URL, server.Client())
	i, err := c.pair(p.Code)
	if err != nil {
		t.Fatal(err)
	}
	lifetime := c.ctx
	setup, cancel := context.WithTimeout(lifetime, 500*time.Millisecond)
	c.ctx = setup
	started := time.Now()
	err = c.tryDirect(i)
	c.ctx = lifetime
	cancel()
	if err == nil {
		t.Fatal("delayed direct negotiation unexpectedly succeeded")
	}
	if time.Since(started) > 2*time.Second {
		t.Fatal("optional negotiation ignored its deadline")
	}
	if _, ok := c.direct.Load(i.DeviceID); ok {
		t.Fatal("timed-out negotiation published a session")
	}
	if err = directRoundTrip(c, i.DeviceID, []byte("relay remains available after signaling timeout")); err != nil {
		t.Fatal(err)
	}
	if err = c.stop(i.DeviceID); err != nil {
		t.Fatalf("stop after signaling timeout: %v", err)
	}
}

func TestDirectControlConnectionLossClosesStreams(t *testing.T) {
	for _, mode := range []string{"controller", "hub"} {
		t.Run(mode, func(t *testing.T) {
			h, server := testHub(t)
			targets, _ := directTestTarget(t)
			p := directTestDevice(t, server.URL, server.Client(), directConfig{loopback: true}, targets)
			c := directTestController(t, server.URL, server.Client())
			i := directTestPair(t, c, p)
			ch, err := c.open(i.DeviceID, "jump")
			if err != nil {
				t.Fatal(err)
			}
			defer ch.Close()
			if mode == "controller" {
				c.transport.Close()
			} else {
				h.Close()
			}
			closed := make(chan error, 1)
			go func() { _, err := io.Copy(io.Discard, ch); closed <- err }()
			select {
			case <-closed:
			case <-time.After(3 * time.Second):
				t.Fatal("control loss did not close active direct stream")
			}
			select {
			case <-c.ctx.Done():
			case <-time.After(time.Second):
				t.Fatal("controller context remains live")
			}
		})
	}
}

func TestDirectSignalBounds(t *testing.T) {
	if _, err := readSignal(bytes.NewReader(bytes.Repeat([]byte(" "), maxSignal+1))); err == nil {
		t.Fatal("oversized signal accepted")
	}
	if _, err := readSignal(bytes.NewBufferString(`{"unknown":true}`)); err == nil {
		t.Fatal("unknown signal field accepted")
	}
	if _, err := readSignal(&directErrorReader{}); !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatalf("signal read error lost: %v", err)
	}
}

type directErrorReader struct{}

func (*directErrorReader) Read([]byte) (int, error) { return 0, io.ErrUnexpectedEOF }
