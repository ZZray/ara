// Copyright 2026 Ray Zhang.
package main

import (
	"archive/tar"
	"bytes"
	"context"
	"encoding/pem"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/http/httputil"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
)

// This runs the ordinary hub binary and the installed OpenSSH client as child
// processes. The controlled TLS proxy and endpoint functions stay in-process:
// Windows' normal TLS verifier has no SSL_CERT_FILE override, and this fixture
// deliberately does not alter the user's system certificate store.
func TestOrdinaryHubProcessDirectAndRelay(t *testing.T) {
	runOrdinaryHubProcess(t, "", []string{"relay", "direct"})
}

func TestLegacyHubProcessFallback(t *testing.T) {
	// Fixed pre-direct source, built in a fresh directory without a checkout or
	// any changes to the shared worktree or its index.
	runOrdinaryHubProcess(t, "6620ad0a9b94d26999b6a74a0ff0847371dac1d1", []string{"legacy"})
}

func runOrdinaryHubProcess(t *testing.T, revision string, modes []string) {
	t.Helper()
	sshBinary, err := exec.LookPath("ssh")
	if err != nil {
		t.Skip("OpenSSH client unavailable")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()
	dir := t.TempDir()
	binary := filepath.Join(dir, "ry-ssh-tunnel")
	goBinary := filepath.Join(runtime.GOROOT(), "bin", "go")
	if runtime.GOOS == "windows" {
		binary += ".exe"
		goBinary += ".exe"
	}
	build := exec.CommandContext(ctx, goBinary, "build", "-buildvcs=false", "-o", binary, ".")
	if revision != "" {
		archive, err := exec.CommandContext(ctx, "git", "-C", "../..", "archive", "--format=tar", revision, "tools/ssh-tunnel").Output()
		if err != nil {
			t.Fatalf("archive fixed legacy hub %s: %v", revision, err)
		}
		source := filepath.Join(dir, "legacy-source")
		reader := tar.NewReader(bytes.NewReader(archive))
		for {
			header, err := reader.Next()
			if err == io.EOF {
				break
			}
			if err != nil {
				t.Fatal(err)
			}
			if header.Typeflag != tar.TypeReg {
				continue
			}
			relative, ok := strings.CutPrefix(header.Name, "tools/ssh-tunnel/")
			if !ok || !filepath.IsLocal(relative) {
				t.Fatalf("unexpected source archive path: %s", header.Name)
			}
			path := filepath.Join(source, filepath.FromSlash(relative))
			if err = os.MkdirAll(filepath.Dir(path), 0700); err != nil {
				t.Fatal(err)
			}
			data, err := io.ReadAll(reader)
			if err != nil {
				t.Fatal(err)
			}
			if err = os.WriteFile(path, data, 0600); err != nil {
				t.Fatal(err)
			}
		}
		build.Dir = source
	}
	if output, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build ordinary hub: %v\n%s", err, output)
	}
	reservation, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	address := reservation.Addr().String()
	reservation.Close()
	output, err := os.Create(filepath.Join(dir, "hub.log"))
	if err != nil {
		t.Fatal(err)
	}
	defer output.Close()
	cmd := exec.CommandContext(ctx, binary, "hub", "-listen", address)
	cmd.Stdout, cmd.Stderr = output, output
	if err = cmd.Start(); err != nil {
		t.Fatal(err)
	}
	processDone := make(chan error, 1)
	go func() { processDone <- cmd.Wait() }()
	defer func() {
		cancel()
		select {
		case <-processDone:
		case <-time.After(5 * time.Second):
			t.Error("ordinary hub did not exit")
		}
	}()
	httpClient := &http.Client{Timeout: time.Second}
	deadline := time.Now().Add(5 * time.Second)
	for {
		response, err := httpClient.Get("http://" + address + "/healthz")
		if err == nil {
			body, _ := io.ReadAll(response.Body)
			response.Body.Close()
			if response.StatusCode == http.StatusOK && string(body) == "ry-ssh-tunnel ok\n" {
				break
			}
		}
		if time.Now().After(deadline) {
			t.Fatal("ordinary hub did not become healthy")
		}
		time.Sleep(20 * time.Millisecond)
	}
	upstream, _ := url.Parse("http://" + address)
	proxy := httptest.NewTLSServer(httputil.NewSingleHostReverseProxy(upstream))
	defer proxy.Close()

	for _, mode := range modes {
		t.Run(mode, func(t *testing.T) {
			endpointCtx, endpointCancel := context.WithCancel(context.WithValue(ctx, directConfigKey{}, directConfig{disabled: mode == "relay", loopback: true}))
			defer endpointCancel()
			fixture := newSSHFixture(t)
			paired := make(chan pairing, 1)
			deviceDone := make(chan error, 1)
			deviceKey := testSigner(t)
			go func() {
				deviceDone <- runDevice(endpointCtx, proxy.URL, "ordinary-hub-"+mode, deviceKey, fixture.authorize, func(p pairing) { paired <- p }, proxy.Client(), false)
			}()
			defer func() {
				endpointCancel()
				select {
				case <-deviceDone:
				case <-time.After(5 * time.Second):
					t.Error("controlled endpoint did not exit")
				}
			}()
			var pair pairing
			select {
			case pair = <-paired:
			case err := <-deviceDone:
				t.Fatalf("controlled registration failed: %v", err)
			case <-time.After(5 * time.Second):
				t.Fatal("controlled registration timed out")
			}
			signer, private, err := newSigner()
			if err != nil {
				t.Fatal(err)
			}
			c, err := newController(endpointCtx, proxy.URL, signer, proxy.Client(), false)
			if err != nil {
				t.Fatal(err)
			}
			defer func() {
				c.transport.Close()
				for _, d := range c.devices {
					d.Close()
				}
			}()
			info, err := c.pair(pair.Code)
			if err != nil {
				t.Fatal(err)
			}
			state := t.TempDir()
			if err = secureDir(state); err != nil {
				t.Fatal(err)
			}
			c.keyPath, c.hostsPath = filepath.Join(state, "key"), filepath.Join(state, "known_hosts")
			block, err := ssh.MarshalPrivateKey(private, "RY ordinary-process test")
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
			value, connectedDirect := c.direct.Load(info.DeviceID)
			if connectedDirect != (mode == "direct") {
				t.Fatalf("expected %s, direct session present=%v", mode, connectedDirect)
			}
			if connectedDirect {
				t.Logf("ordinary hub negotiated %s", value.(*directSession).path)
			}
			for _, target := range []string{"jump", "backend"} {
				commandCtx, commandCancel := context.WithTimeout(endpointCtx, 10*time.Second)
				args := append(c.sshArgs(c.devices[n].endpoints[target]), "probe")
				result, commandErr := exec.CommandContext(commandCtx, sshBinary, args...).CombinedOutput()
				commandCancel()
				if commandErr != nil || !strings.Contains(string(result), "RY fixture probe OK") {
					t.Fatalf("ordinary SSH %s/%s: %v\n%s", mode, target, commandErr, result)
				}
			}
			active, err := c.open(info.DeviceID, "jump")
			if err != nil {
				t.Fatal(err)
			}
			defer active.Close()
			closed := make(chan struct{})
			go func() { io.Copy(io.Discard, active); close(closed) }()
			if mode != "direct" {
				if err = c.stop(info.DeviceID); err != nil {
					t.Fatal(err)
				}
			} else {
				// Terminating the real hub process must revoke already established
				// peer-to-peer access, despite data bypassing that process.
				if err = cmd.Process.Kill(); err != nil {
					t.Fatal(err)
				}
			}
			select {
			case <-closed:
			case <-time.After(3 * time.Second):
				t.Fatal("active SSH stream survived session revocation")
			}
			if mode == "direct" {
				select {
				case <-c.ctx.Done():
				case <-time.After(3 * time.Second):
					t.Fatal("controller context survived ordinary hub process exit")
				}
			}
			t.Logf("ordinary hub PID %d: %s jump/backend via external SSH, active-stream revocation passed; endpoints/TLS proxy remain in-process", cmd.Process.Pid, mode)
		})
	}
}
