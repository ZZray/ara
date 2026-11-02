// Copyright 2026 Ray Zhang.
package main

import (
	"context"
	"errors"
	"io"
	"net/http"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
)

// A closed completion channel is observable both by assertions and cleanup.
// Unlike consuming an error channel twice, this also preserves the actual
// runDevice return value for the lifecycle assertion.
type lifecycleDevice struct {
	pair pairing
	done chan struct{}
	err  error // read only after done closes
}

func startLifecycleDevice(t *testing.T, server string, client *http.Client, auth authorizeFunc) *lifecycleDevice {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.WithValue(context.Background(), directConfigKey{}, directConfig{loopback: true}), 10*time.Second)
	signer := testSigner(t)
	run := &lifecycleDevice{done: make(chan struct{})}
	pairs := make(chan pairing, 1)
	go func() {
		run.err = runDevice(ctx, server, "lifecycle fixture", signer, auth, func(p pairing) { pairs <- p }, client, false)
		close(run.done)
	}()
	t.Cleanup(func() {
		cancel()
		select {
		case <-run.done:
		case <-time.After(3 * time.Second):
			t.Error("runDevice did not finish cleanup")
		}
	})
	select {
	case run.pair = <-pairs:
	case <-run.done:
		t.Fatalf("registration failed: %v", run.err)
	case <-time.After(3 * time.Second):
		t.Fatal("registration timed out")
	}
	return run
}

func TestDeviceLifecycleCompletedSessionReturnsNil(t *testing.T) {
	for _, path := range []string{"relay", "direct"} {
		for _, ending := range []string{"stop", "owner-close", "hub-close"} {
			t.Run(path+"/"+ending, func(t *testing.T) {
				h, server := testHub(t)
				targets, _ := directTestTarget(t)
				run := startLifecycleDevice(t, server.URL, server.Client(), func(context.Context, ssh.PublicKey) (authorization, error) {
					return authorization{OK: true, Targets: targets}, nil
				})
				c := directTestController(t, server.URL, server.Client())
				info, err := c.pair(run.pair.Code)
				if err != nil {
					t.Fatal(err)
				}
				if path == "direct" {
					if err = c.tryDirect(info); err != nil {
						t.Fatal(err)
					}
				}
				active, err := c.open(info.DeviceID, "jump")
				if err != nil {
					t.Fatal(err)
				}
				defer active.Close()
				if _, err = active.Write([]byte("active before session termination")); err != nil {
					t.Fatal(err)
				}
				streamDone := make(chan struct{})
				go func() { io.Copy(io.Discard, active); close(streamDone) }()
				switch ending {
				case "stop":
					if err = c.stop(info.DeviceID); err != nil {
						t.Fatal(err)
					}
				case "owner-close":
					c.transport.Close()
				case "hub-close":
					h.Close()
				}
				select {
				case <-run.done:
					if run.err != nil {
						t.Errorf("finished session returned an error: %v", run.err)
					}
				case <-time.After(3 * time.Second):
					t.Fatal("session did not finish")
				}
				select {
				case <-streamDone:
				case <-time.After(3 * time.Second):
					t.Fatal("session ended with an active stream still open")
				}
			})
		}
	}
}

func TestDeviceLifecycleAuthorizationFailureRemainsError(t *testing.T) {
	_, server := testHub(t)
	denied := errors.New("fixture refused local authorization")
	run := startLifecycleDevice(t, server.URL, server.Client(), func(context.Context, ssh.PublicKey) (authorization, error) {
		return authorization{}, denied
	})
	c := directTestController(t, server.URL, server.Client())
	if _, err := c.pair(run.pair.Code); err == nil {
		t.Fatal("pairing accepted a failed local authorization")
	}
	select {
	case <-run.done:
		if !errors.Is(run.err, denied) {
			t.Fatalf("authorization failure was lost: got %v", run.err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("failed authorization did not end runDevice")
	}
}
