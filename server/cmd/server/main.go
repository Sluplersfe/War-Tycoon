// Command server runs the Frontline Foundry relay: TCP 4002, UDP 4003.
package main

import (
	"context"
	"flag"
	"log/slog"
	"net"
	"os"
	"os/signal"
	"strconv"
	"syscall"
	"time"

	"frontline/internal/relay"
	"frontline/internal/rooms"
	"frontline/internal/store"
	"frontline/internal/world"
)

func env(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func main() {
	tcpAddr := flag.String("tcp", env("FF_TCP_ADDR", ":4002"), "TCP listen address")
	udpAddr := flag.String("udp", env("FF_UDP_ADDR", ":4003"), "UDP listen address")
	sharedDir := flag.String("shared", env("FF_SHARED_DIR", "../shared"), "directory with map.json and vehicles.json")
	memory := flag.Bool("memory", false, "use an in-memory store instead of Postgres (dev only)")
	health := flag.Bool("healthcheck", false, "check that the TCP port accepts connections and exit")
	flag.Parse()

	if *health {
		c, err := net.DialTimeout("tcp", "127.0.0.1"+*tcpAddr, 2*time.Second)
		if err != nil {
			os.Exit(1)
		}
		c.Close()
		return
	}

	log := slog.New(slog.NewTextHandler(os.Stdout, &slog.HandlerOptions{Level: slog.LevelInfo}))
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	w, err := world.Load(*sharedDir)
	if err != nil {
		log.Error("load world", "err", err)
		os.Exit(1)
	}

	var st store.Store
	if *memory {
		log.Warn("using in-memory store; nothing will be saved")
		st = store.NewMemory()
	} else {
		url := os.Getenv("DATABASE_URL")
		if url == "" {
			log.Error("DATABASE_URL is not set (or pass -memory)")
			os.Exit(1)
		}
		pg, err := store.OpenPostgres(ctx, url)
		if err != nil {
			log.Error("open database", "err", err)
			os.Exit(1)
		}
		defer pg.Close()
		st = pg
	}

	cfg := rooms.DefaultConfig()
	if v, err := strconv.Atoi(os.Getenv("FF_TICK_RATE")); err == nil && v > 0 {
		cfg.TickRate = v
	}
	rm := rooms.NewManager(st, w, cfg, log)
	srv := relay.New(st, rm, log)
	if err := srv.Listen(*tcpAddr, *udpAddr); err != nil {
		log.Error("listen", "err", err)
		os.Exit(1)
	}
	log.Info("listening", "tcp", srv.TCPAddr(), "udp", srv.UDPAddr(), "map", w.Map.Name)
	srv.Serve(ctx)

	log.Info("shutting down, saving rooms")
	sctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	rm.Shutdown(sctx)
}
