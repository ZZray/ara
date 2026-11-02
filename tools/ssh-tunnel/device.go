// Copyright 2026 Ray Zhang.
package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"time"

	"golang.org/x/crypto/ssh"
)

type authorizeFunc func(context.Context, ssh.PublicKey) (authorization, error)

func fileAuthorizer(dir string) authorizeFunc {
	return func(ctx context.Context, key ssh.PublicKey) (authorization, error) {
		if err := publishFile(filepath.Join(dir, "controller.pub"), ssh.MarshalAuthorizedKey(key)); err != nil {
			return authorization{}, err
		}
		deadline, cancel := context.WithTimeout(ctx, authorizationTimeout)
		defer cancel()
		ticker := time.NewTicker(250 * time.Millisecond)
		defer ticker.Stop()
		for {
			select {
			case <-deadline.Done():
				return authorization{}, errors.New("local SSH authorization did not finish within three minutes")
			case <-ticker.C:
				path := filepath.Join(dir, "authorized.json")
				info, err := os.Stat(path)
				if errors.Is(err, os.ErrNotExist) {
					continue
				}
				if err != nil {
					return authorization{}, err
				}
				if info.Size() > 64*1024 {
					return authorization{}, errors.New("authorization receipt is too large")
				}
				data, err := os.ReadFile(path)
				if err != nil {
					return authorization{}, err
				}
				var receipt authorization
				if err = decode(data, &receipt); err != nil {
					return authorization{}, fmt.Errorf("authorization receipt: %w", err)
				}
				if !receipt.OK {
					return authorization{}, fmt.Errorf("local SSH authorization failed: %s", receipt.Error)
				}
				if err = validateTargets(receipt.Targets); err != nil {
					return authorization{}, err
				}
				return receipt, nil
			}
		}
	}
}

func runDevice(ctx context.Context, server, name string, signer ssh.Signer, authorize authorizeFunc, paired func(pairing), hc *http.Client, allowHTTP bool) error {
	t, err := dialTransport(ctx, server, "device", signer, hc, allowHTTP)
	if err != nil {
		return err
	}
	defer t.Close()
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	stop := context.AfterFunc(ctx, func() { t.Close() })
	defer stop()
	response, err := request(t.Conn, "ry-register", encode(struct {
		Name string `json:"name"`
	}{name}), handshakeTimeout)
	if err != nil {
		return err
	}
	var p pairing
	if err = decode(response, &p); err != nil {
		return err
	}
	paired(p)
	var mu sync.Mutex
	var targets map[string]target
	var owner ssh.PublicKey
	directAttempted := false
	var workers sync.WaitGroup
	defer func() { cancel(); t.Close(); workers.Wait() }()
	slots := make(chan struct{}, maxStreams)
	// Relay and direct SSH use the same target authorization and stream budget.
	serveTarget := func(incoming ssh.NewChannel) {
		name := string(incoming.ExtraData())
		mu.Lock()
		selected, ok := targets[name]
		mu.Unlock()
		if incoming.ChannelType() != "ry-target" || !ok {
			incoming.Reject(ssh.Prohibited, "target not authorized")
			return
		}
		select {
		case slots <- struct{}{}:
			workers.Go(func() {
				defer func() { <-slots }()
				local, err := (&net.Dialer{Timeout: 5 * time.Second}).DialContext(ctx, "tcp", selected.Address)
				if err != nil {
					incoming.Reject(ssh.ConnectionFailed, "local target unavailable")
					return
				}
				defer local.Close()
				channel, requests, err := incoming.Accept()
				if err != nil {
					return
				}
				go ssh.DiscardRequests(requests)
				copyStreams(ctx, channel, local)
			})
		default:
			incoming.Reject(ssh.ResourceShortage, "stream limit")
		}
	}
	workers.Go(func() {
		for incoming := range t.Channels {
			if incoming.ChannelType() == directProtocol {
				mu.Lock()
				key := owner
				allowed := key != nil && !directAttempted && !directSettings(ctx).disabled
				if allowed {
					directAttempted = true
				}
				mu.Unlock()
				if !allowed {
					incoming.Reject(ssh.Prohibited, "direct unavailable")
					continue
				}
				workers.Go(func() { serveDirect(ctx, incoming, signer, key, serveTarget) })
			} else {
				serveTarget(incoming)
			}
		}
	})
	// Conn.Wait is also needed while the local authorizer is waiting for PowerShell.
	workers.Go(func() { t.Conn.Wait(); cancel() })
	authorized := false
	for req := range t.Requests {
		if req.Type != "ry-authorize" || authorized {
			req.Reply(false, []byte("unsupported request"))
			continue
		}
		authorized = true
		key, err := publicKey(string(req.Payload))
		if err != nil {
			req.Reply(false, []byte("invalid controller key"))
			return err
		}
		receipt, err := authorize(ctx, key)
		if err == nil {
			err = validateTargets(receipt.Targets)
		}
		if err != nil {
			req.Reply(false, []byte("local authorization failed"))
			return err
		}
		mu.Lock()
		targets = receipt.Targets
		owner = key
		mu.Unlock()
		if err = req.Reply(true, encode(receipt)); err != nil {
			return err
		}
		log.Printf("READY: paired controller authorized; targets=%d. Closing this session removes its cloud access.", len(receipt.Targets))
	}
	if ctx.Err() != nil {
		return nil
	}
	return errors.New("relay connection closed; this one-time session ended")
}

func deviceCommand(ctx context.Context, server, name, dir string) error {
	if dir == "" {
		return errors.New("-state-dir is required; launch the controlled-side script")
	}
	if err := os.MkdirAll(dir, 0700); err != nil {
		return err
	}
	if err := secureDir(dir); err != nil {
		return err
	}
	lock := filepath.Join(dir, "device.lock")
	if err := writeNewFile(lock, []byte(fmt.Sprint(os.Getpid()))); err != nil {
		return fmt.Errorf("session directory is already in use: %w", err)
	}
	defer os.Remove(lock)
	for _, name := range []string{"pairing.json", "controller.pub", "authorized.json"} {
		if _, err := os.Stat(filepath.Join(dir, name)); !errors.Is(err, os.ErrNotExist) {
			return errors.New("use a fresh state directory for every controlled session")
		}
	}
	signer, _, err := newSigner()
	if err != nil {
		return err
	}
	var writeErr error
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	err = runDevice(ctx, server, name, signer, fileAuthorizer(dir), func(p pairing) {
		writeErr = publishFile(filepath.Join(dir, "pairing.json"), encode(p))
		if writeErr == nil {
			fmt.Printf("Pairing code: %s\nValid until: %s\n", p.Code, p.ExpiresAt.Local().Format(time.RFC3339))
		} else {
			cancel()
		}
	}, nil, false)
	return errors.Join(err, writeErr)
}
