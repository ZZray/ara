// Copyright 2026 Ray Zhang.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/pion/stun/v4"
)

var version = "development"

func run(ctx context.Context, args []string) error {
	if len(args) == 0 {
		return errors.New("usage: ry-ssh-tunnel hub|device|controller|version")
	}
	flags := flag.NewFlagSet(args[0], flag.ContinueOnError)
	var direct *bool
	var stunServers *string
	if args[0] == "device" || args[0] == "controller" {
		direct = flags.Bool("direct", true, "try ICE UDP direct transport before relay")
		stunServers = flags.String("stun", strings.Join(directSettings(ctx).stun, ","), "comma-separated STUN URLs; empty uses local candidates only")
	}
	configureDirect := func() error {
		cfg := directConfig{disabled: !*direct}
		if *stunServers != "" {
			for _, raw := range strings.Split(*stunServers, ",") {
				u, err := stun.ParseURI(strings.TrimSpace(raw))
				if err != nil || u.Scheme != stun.SchemeTypeSTUN {
					return errors.New("-stun accepts only stun: URLs; TURN relays are not used")
				}
				cfg.stun = append(cfg.stun, u.String())
			}
		}
		ctx = context.WithValue(ctx, directConfigKey{}, cfg)
		return nil
	}
	switch args[0] {
	case "version":
		fmt.Println("RY SSH Tunnel", version)
		return nil
	case "hub":
		listen := flags.String("listen", "127.0.0.1:50122", "loopback HTTP address; put behind HTTPS reverse proxy")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		host, _, err := net.SplitHostPort(*listen)
		if err != nil {
			return err
		}
		ip := net.ParseIP(host)
		if ip == nil || !ip.IsLoopback() {
			return errors.New("hub must bind a loopback address behind HTTPS")
		}
		h, err := newHub()
		if err != nil {
			return err
		}
		defer h.Close()
		server := &http.Server{Addr: *listen, Handler: h, ReadHeaderTimeout: 5 * time.Second, IdleTimeout: 30 * time.Second, MaxHeaderBytes: 8192}
		done := make(chan error, 1)
		go func() { done <- server.ListenAndServe() }()
		log.Printf("RY independent SSH relay %s listening on %s", version, *listen)
		select {
		case err := <-done:
			if errors.Is(err, http.ErrServerClosed) {
				return nil
			}
			return err
		case <-ctx.Done():
		}
		h.Close()
		shutdown, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		return server.Shutdown(shutdown)
	case "device":
		server := flags.String("server", defaultServer, "HTTPS relay URL")
		dir := flags.String("state-dir", "", "fresh protected session directory")
		hostname, _ := os.Hostname()
		name := flags.String("name", hostname, "visible machine name")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		if err := configureDirect(); err != nil {
			return err
		}
		return deviceCommand(ctx, *server, *name, *dir)
	case "controller":
		server := flags.String("server", defaultServer, "HTTPS relay URL")
		dir := flags.String("state-dir", "", "parent directory for temporary keys (default: OS temp)")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		if err := configureDirect(); err != nil {
			return err
		}
		return controllerCommand(ctx, *server, *dir)
	default:
		return errors.New("unknown command")
	}
}

func main() {
	log.SetOutput(os.Stdout)
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	if err := run(ctx, os.Args[1:]); err != nil {
		log.Print(err)
		os.Exit(1)
	}
}
