// Copyright 2026 Ray Zhang.
package main

import (
	"bufio"
	"context"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"golang.org/x/crypto/ssh"
)

type localEndpoint struct {
	listener net.Listener
	target   target
	alias    string
}
type controlledDevice struct {
	info      deviceInfo
	endpoints map[string]*localEndpoint
	cancel    context.CancelFunc
	workers   sync.WaitGroup
}
type controller struct {
	signer    ssh.Signer
	direct    sync.Map // device ID -> *directSession; read by concurrent forwarding workers
	transport *transport
	ctx       context.Context
	devices   map[int]*controlledDevice
	next      int
	keyPath   string
	hostsPath string
}

func newController(ctx context.Context, server string, signer ssh.Signer, hc *http.Client, allowHTTP bool) (*controller, error) {
	ctx, cancel := context.WithCancel(ctx)
	t, err := dialTransport(ctx, server, "controller", signer, hc, allowHTTP)
	if err != nil {
		cancel()
		return nil, err
	}
	go ssh.DiscardRequests(t.Requests)
	go func() {
		for ch := range t.Channels {
			ch.Reject(ssh.Prohibited, "unsupported channel")
		}
	}()
	go func() { t.Conn.Wait(); cancel() }()
	return &controller{transport: t, ctx: ctx, signer: signer, devices: map[int]*controlledDevice{}, next: 1}, nil
}

func (c *controller) pair(code string) (deviceInfo, error) {
	code = strings.ToUpper(strings.ReplaceAll(strings.TrimSpace(code), "-", ""))
	response, err := request(c.transport.Conn, "ry-pair", encode(struct {
		Code string `json:"code"`
	}{code}), authorizationTimeout+20*time.Second)
	if err != nil {
		return deviceInfo{}, err
	}
	var info deviceInfo
	if err = decode(response, &info); err != nil {
		return info, err
	}
	if err = validateTargets(info.Targets); err != nil {
		return info, err
	}
	return info, nil
}

func (c *controller) stop(id string) error {
	_, err := request(c.transport.Conn, "ry-stop", encode(struct {
		DeviceID string `json:"device_id"`
	}{id}), handshakeTimeout)
	if err == nil {
		if value, ok := c.direct.LoadAndDelete(id); ok {
			value.(*directSession).peer.Close()
		}
	}
	return err
}

func (c *controller) open(id, name string) (ssh.Channel, error) {
	if value, ok := c.direct.Load(id); ok {
		s := value.(*directSession)
		select {
		case <-s.done: // only a new stream may fall back; never replay an attempted open
		default:
			timer := time.AfterFunc(handshakeTimeout, func() { s.peer.Close() })
			defer timer.Stop()
			ch, requests, err := s.conn.OpenChannel("ry-target", []byte(name))
			if err == nil {
				go ssh.DiscardRequests(requests)
			}
			return ch, err
		}
	}
	timer := time.AfterFunc(handshakeTimeout+time.Second, func() { c.transport.Close() })
	defer timer.Stop()
	channel, requests, err := c.transport.Conn.OpenChannel("ry-forward", encode(openRequest{DeviceID: id, Target: name}))
	if err != nil {
		return nil, err
	}
	go ssh.DiscardRequests(requests)
	return channel, nil
}

func (d *controlledDevice) Close() {
	d.cancel()
	for _, e := range d.endpoints {
		e.listener.Close()
	}
	d.workers.Wait()
}

func (c *controller) expose(info deviceInfo) (int, error) {
	if err := c.tryDirect(info); err != nil {
		fmt.Printf("PATH device=%s relay; direct unavailable: %v\n", info.DeviceID, err)
	}
	ctx, cancel := context.WithCancel(c.ctx)
	d := &controlledDevice{info: info, endpoints: map[string]*localEndpoint{}, cancel: cancel}
	ready := false
	defer func() {
		if !ready {
			d.Close()
		}
	}()
	var lines strings.Builder
	for name, t := range info.Targets {
		listener, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			return 0, err
		}
		e := &localEndpoint{listener: listener, target: t, alias: info.DeviceID + "-" + name}
		d.endpoints[name] = e
		key, err := publicKey(t.HostKey)
		if err != nil {
			return 0, err
		}
		fmt.Fprintf(&lines, "%s %s", e.alias, ssh.MarshalAuthorizedKey(key))
	}
	f, err := os.OpenFile(c.hostsPath, os.O_APPEND|os.O_WRONLY, 0600)
	if err != nil {
		return 0, err
	}
	_, err = f.WriteString(lines.String())
	err = errors.Join(err, f.Close())
	if err != nil {
		return 0, err
	}
	for name, e := range d.endpoints {
		d.workers.Go(func() {
			slots := make(chan struct{}, maxStreams)
			for {
				local, err := e.listener.Accept()
				if err != nil {
					return
				}
				select {
				case slots <- struct{}{}:
					d.workers.Go(func() {
						defer func() { <-slots }()
						defer local.Close()
						remote, err := c.open(info.DeviceID, name)
						if err != nil {
							return
						}
						copyStreams(ctx, local, remote)
					})
				default:
					local.Close()
				}
			}
		})
	}
	n := c.next
	c.next++
	c.devices[n] = d
	ready = true
	return n, nil
}

func (c *controller) refresh() error {
	response, err := request(c.transport.Conn, "ry-list", nil, handshakeTimeout)
	if err != nil {
		return err
	}
	var current []deviceInfo
	if err = decode(response, &current); err != nil {
		return err
	}
	live := map[string]bool{}
	for _, info := range current {
		live[info.DeviceID] = true
	}
	for n, d := range c.devices {
		if !live[d.info.DeviceID] {
			d.Close()
			delete(c.devices, n)
		}
	}
	return nil
}

func (c *controller) sshArgs(e *localEndpoint) []string {
	_, port, _ := net.SplitHostPort(e.listener.Addr().String())
	null := "/dev/null"
	if runtime.GOOS == "windows" {
		null = "NUL"
	}
	// -o is parsed again using ssh_config syntax even though exec passes one
	// argument. Quote spaces, preserve literal escapes, and avoid % token expansion.
	hosts := strings.NewReplacer(`\`, `\\`, `"`, `\"`, `%`, `%%`).Replace(filepath.ToSlash(c.hostsPath))
	identity := strings.NewReplacer(`\`, `\\`, `"`, `\"`, `%`, `%%`).Replace(filepath.ToSlash(c.keyPath))
	return []string{"-F", null, "-o", `IdentityFile="` + identity + `"`, "-o", "IdentitiesOnly=yes", "-o", "StrictHostKeyChecking=yes", "-o", `UserKnownHostsFile="` + hosts + `"`, "-o", "GlobalKnownHostsFile=" + null, "-o", "HostKeyAlias=" + e.alias, "-p", port, "-l", e.target.User, "127.0.0.1"}
}

func shellQuote(s string) string { return "'" + strings.ReplaceAll(s, "'", "''") + "'" }

func (c *controller) printDevice(n int) {
	d := c.devices[n]
	fmt.Printf("[%d] %s  id=%s\n", n, d.info.Name, d.info.DeviceID)
	path := "relay (SSH/file data via hub)"
	if value, ok := c.direct.Load(d.info.DeviceID); ok {
		s := value.(*directSession)
		select {
		case <-s.done:
		default:
			path = s.path + " (SSH/file data bypass hub)"
		}
	}
	fmt.Printf("  Path: %s\n", path)
	for _, name := range []string{"jump", "backend"} {
		e := d.endpoints[name]
		if e == nil {
			continue
		}
		fmt.Printf("  %s: %s, user=%s\n", name, e.listener.Addr(), e.target.User)
		args := c.sshArgs(e)
		quoted := make([]string, len(args))
		for i, a := range args {
			quoted[i] = shellQuote(a)
		}
		fmt.Printf("  ssh %s\n", strings.Join(quoted, " "))
	}
}

func controllerCommand(ctx context.Context, server, parent string) (result error) {
	if parent != "" {
		if err := os.MkdirAll(parent, 0700); err != nil {
			return err
		}
	}
	dir, err := os.MkdirTemp(parent, "ry-controller-")
	if err != nil {
		return err
	}
	// Only this freshly allocated directory is removed. It never contains a user's
	// existing SSH keys; a new private key is created for this support window.
	defer func() { result = errors.Join(result, os.RemoveAll(dir)) }()
	if err = secureDir(dir); err != nil {
		return err
	}
	signer, private, err := newSigner()
	if err != nil {
		return err
	}
	block, err := ssh.MarshalPrivateKey(private, "RY temporary support")
	if err != nil {
		return err
	}
	keyPath := filepath.Join(dir, "controller.key")
	if err = writeNewFile(keyPath, pem.EncodeToMemory(block)); err != nil {
		return err
	}
	hostsPath := filepath.Join(dir, "known_hosts")
	if err = writeNewFile(hostsPath, nil); err != nil {
		return err
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	c, err := newController(ctx, server, signer, nil, false)
	if err != nil {
		return err
	}
	c.keyPath = keyPath
	c.hostsPath = hostsPath
	defer func() {
		c.transport.Close()
		for _, d := range c.devices {
			d.Close()
		}
	}()
	go func() { c.transport.Conn.Wait(); cancel() }()
	fmt.Printf("RY SSH Tunnel controller %s\nRelay: %s\n", version, server)
	fmt.Println("Commands: add <pairing-code> | list | ssh <number> jump/backend | stop <number> | quit")
	fmt.Println("Keep this window open. quit ends all of this controller's controlled sessions.")
	fmt.Printf("Temporary SSH key: %s\nHost keys: %s\n", keyPath, hostsPath)
	reader := bufio.NewReader(os.Stdin)
	for {
		fmt.Print("ry> ")
		type readResult struct {
			line string
			err  error
		}
		input := make(chan readResult, 1)
		go func() { line, err := reader.ReadString('\n'); input <- readResult{line, err} }()
		var line string
		select {
		case <-ctx.Done():
			fmt.Println("\nSession ended; controlled-side scripts will clean up their temporary authorization.")
			return nil
		case r := <-input:
			if r.err != nil && errors.Is(r.err, io.EOF) {
				return nil
			}
			if r.err != nil {
				return r.err
			}
			line = r.line
		}
		parts := strings.Fields(line)
		if len(parts) == 0 {
			continue
		}
		switch parts[0] {
		case "quit", "exit":
			return nil
		case "add":
			if len(parts) != 2 {
				fmt.Println("Usage: add <pairing-code>")
				continue
			}
			fmt.Println("Waiting for controlled-side SSH authorization...")
			info, err := c.pair(parts[1])
			if err != nil {
				fmt.Println(err)
				continue
			}
			n, err := c.expose(info)
			if err != nil {
				c.stop(info.DeviceID)
				fmt.Println(err)
				continue
			}
			c.printDevice(n)
		case "list":
			if err := c.refresh(); err != nil {
				fmt.Println(err)
				continue
			}
			ids := make([]int, 0, len(c.devices))
			for n := range c.devices {
				ids = append(ids, n)
			}
			sort.Ints(ids)
			if len(ids) == 0 {
				fmt.Println("No paired devices.")
			}
			for _, n := range ids {
				c.printDevice(n)
			}
		case "stop", "ssh":
			if len(parts) < 2 {
				fmt.Println("Device number is required.")
				continue
			}
			n, err := strconv.Atoi(parts[1])
			d := c.devices[n]
			if err != nil || d == nil {
				fmt.Println("Unknown device number.")
				continue
			}
			if parts[0] == "stop" {
				if err = c.stop(d.info.DeviceID); err != nil {
					fmt.Println(err)
					continue
				}
				d.Close()
				delete(c.devices, n)
				fmt.Println("Tunnel disconnected. Controlled-side cleanup runs locally; its completion is not confirmed by this response.")
				continue
			}
			if len(parts) != 3 || d.endpoints[parts[2]] == nil {
				fmt.Println("Usage: ssh <number> jump/backend")
				continue
			}
			binary, err := exec.LookPath("ssh")
			if err != nil {
				fmt.Println("OpenSSH client is missing from PATH.")
				continue
			}
			command := exec.CommandContext(ctx, binary, c.sshArgs(d.endpoints[parts[2]])...)
			command.Stdin = os.Stdin
			command.Stdout = os.Stdout
			command.Stderr = os.Stderr
			if err = command.Run(); err != nil {
				fmt.Printf("SSH process: %v\n", err)
			}
		default:
			fmt.Println("Commands: add, list, ssh, stop, quit")
		}
	}
}
