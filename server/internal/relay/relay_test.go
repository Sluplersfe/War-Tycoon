package relay

import (
	"context"
	"io"
	"log/slog"
	"net"
	"testing"
	"time"

	"google.golang.org/protobuf/proto"

	"frontline/internal/pb"
	"frontline/internal/rooms"
	"frontline/internal/store"
	"frontline/internal/world"
)

type client struct {
	t     *testing.T
	tcp   net.Conn
	udp   *net.UDPConn
	msgs  chan *pb.ServerMsg
	udpIn chan *pb.UdpPacket
	conn  uint32
	key   uint64
	user  uint64
}

func startServer(t *testing.T) (*Server, store.Store, func()) {
	t.Helper()
	w, err := world.Load("../../../shared")
	if err != nil {
		t.Fatal(err)
	}
	st := store.NewMemory()
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	cfg := rooms.DefaultConfig()
	cfg.IdleUnload = 200 * time.Millisecond
	rm := rooms.NewManager(st, w, cfg, log)
	srv := New(st, rm, log)
	if err := srv.Listen("127.0.0.1:0", "127.0.0.1:0"); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { srv.Serve(ctx); rm.Shutdown(context.Background()); close(done) }()
	return srv, st, func() { cancel(); <-done }
}

func dial(t *testing.T, srv *Server) *client {
	t.Helper()
	tc, err := net.Dial("tcp", srv.TCPAddr().String())
	if err != nil {
		t.Fatal(err)
	}
	uc, err := net.DialUDP("udp", nil, srv.UDPAddr().(*net.UDPAddr))
	if err != nil {
		t.Fatal(err)
	}
	c := &client{t: t, tcp: tc, udp: uc, msgs: make(chan *pb.ServerMsg, 256), udpIn: make(chan *pb.UdpPacket, 256)}
	go func() {
		for {
			m := &pb.ServerMsg{}
			if err := ReadFrame(tc, m); err != nil {
				close(c.msgs)
				return
			}
			c.msgs <- m
		}
	}()
	go func() {
		buf := make([]byte, 2048)
		for {
			n, err := uc.Read(buf)
			if err != nil {
				return
			}
			p := &pb.UdpPacket{}
			if proto.Unmarshal(buf[:n], p) == nil {
				c.udpIn <- p
			}
		}
	}()
	t.Cleanup(func() { tc.Close(); uc.Close() })
	return c
}

func (c *client) send(m *pb.ClientMsg) {
	if err := WriteFrame(c.tcp, m); err != nil {
		c.t.Fatal(err)
	}
}

// expect waits for the first message matching pred, skipping others.
func (c *client) expect(what string, pred func(*pb.ServerMsg) bool) *pb.ServerMsg {
	c.t.Helper()
	timeout := time.After(3 * time.Second)
	for {
		select {
		case m, ok := <-c.msgs:
			if !ok {
				c.t.Fatalf("connection closed waiting for %s", what)
			}
			if e := m.GetError(); e != nil && what != "error" {
				c.t.Logf("server error: %s: %s", e.Code, e.Message)
			}
			if pred(m) {
				return m
			}
		case <-timeout:
			c.t.Fatalf("timeout waiting for %s", what)
		}
	}
}

func (c *client) expectUDP(what string, pred func(*pb.UdpPacket) bool) *pb.UdpPacket {
	c.t.Helper()
	timeout := time.After(2 * time.Second)
	for {
		select {
		case p := <-c.udpIn:
			if pred(p) {
				return p
			}
		case <-timeout:
			c.t.Fatalf("timeout waiting for udp %s", what)
		}
	}
}

func (c *client) noUDP(what string, pred func(*pb.UdpPacket) bool) {
	c.t.Helper()
	timeout := time.After(300 * time.Millisecond)
	for {
		select {
		case p := <-c.udpIn:
			if pred(p) {
				c.t.Fatalf("unexpected udp %s", what)
			}
		case <-timeout:
			return
		}
	}
}

func (c *client) udpSend(p *pb.UdpPacket) {
	p.ConnId, p.Key = c.conn, c.key
	b, _ := proto.Marshal(p)
	c.udp.Write(b)
}

func (c *client) register(name string) {
	c.send(&pb.ClientMsg{Msg: &pb.ClientMsg_Register{Register: &pb.Register{Username: name, Password: "password123"}}})
	m := c.expect("auth_ok", func(m *pb.ServerMsg) bool { return m.GetAuthOk() != nil })
	c.user = m.GetAuthOk().UserId
}

func (c *client) joined(m *pb.ServerMsg) *pb.RoomJoined {
	j := m.GetRoomJoined()
	c.conn, c.key = j.ConnId, j.UdpKey
	c.udpSend(&pb.UdpPacket{Hello: true})
	c.expectUDP("hello ack", func(p *pb.UdpPacket) bool { return p.Hello })
	return j
}

func isJoined(m *pb.ServerMsg) bool { return m.GetRoomJoined() != nil }

func TestEndToEnd(t *testing.T) {
	srv, st, stop := startServer(t)
	defer stop()

	alice, bob := dial(t, srv), dial(t, srv)
	alice.register("alice")
	bob.register("bob")

	alice.send(&pb.ClientMsg{Msg: &pb.ClientMsg_CreateRoom{CreateRoom: &pb.CreateRoom{Name: "Test room"}}})
	aj := alice.joined(alice.expect("room_joined", isJoined))
	if !aj.You.Host || aj.Settings.MaxPlayers == 0 || aj.You.AvatarId == 0 {
		t.Fatalf("creator is host with defaults: %+v", aj.You)
	}
	roomID := aj.Room.Id

	bob.send(&pb.ClientMsg{Msg: &pb.ClientMsg_JoinRoom{JoinRoom: &pb.JoinRoom{RoomId: roomID}}})
	bj := bob.joined(bob.expect("room_joined", isJoined))
	if bj.You.Plot == aj.You.Plot {
		t.Fatal("players get different plots")
	}
	if bj.You.Team == aj.You.Team {
		t.Fatal("auto team balancing")
	}
	alice.expect("player_joined", func(m *pb.ServerMsg) bool { return m.GetPlayerJoined() != nil })

	// Owner's update is relayed.
	aliceAvatar := aj.You.AvatarId
	alice.udpSend(&pb.UdpPacket{Updates: []*pb.ObjectUpdate{{
		Id: aliceAvatar, Epoch: 1, State: &pb.ObjectState{Pos: &pb.Vec3{X: 42}, Rot: &pb.Quat{W: 1}},
	}}})
	bob.expectUDP("alice avatar", func(p *pb.UdpPacket) bool {
		return len(p.Updates) == 1 && p.Updates[0].Id == aliceAvatar && p.Updates[0].State.Pos.X == 42
	})

	// Bob may not send state for alice's character.
	bob.udpSend(&pb.UdpPacket{Updates: []*pb.ObjectUpdate{{
		Id: aliceAvatar, Epoch: 1, State: &pb.ObjectState{Pos: &pb.Vec3{X: -1}, Rot: &pb.Quat{W: 1}},
	}}})
	alice.noUDP("forged update", func(p *pb.UdpPacket) bool {
		for _, u := range p.Updates {
			if u.Id == aliceAvatar {
				return true
			}
		}
		return false
	})

	// Alice spawns a jeep unowned; server-owned objects are simulated and broadcast.
	alice.send(&pb.ClientMsg{Msg: &pb.ClientMsg_Spawn{Spawn: &pb.SpawnObject{
		Kind: pb.ObjectKind_OBJECT_KIND_VEHICLE, Def: "jeep", Unowned: true,
		State: &pb.ObjectState{Pos: &pb.Vec3{X: 200, Y: 5, Z: 0}, Rot: &pb.Quat{W: 1}},
	}}})
	sp := bob.expect("object_spawned", func(m *pb.ServerMsg) bool {
		return m.GetObjectSpawned() != nil && m.GetObjectSpawned().Object.Def == "jeep"
	}).GetObjectSpawned().Object
	if sp.Owner != 0 {
		t.Fatal("unowned spawn belongs to the server")
	}
	bob.expectUDP("server sim of falling jeep", func(p *pb.UdpPacket) bool {
		for _, u := range p.Updates {
			if u.Id == sp.Id && u.State.Pos.Y < 5 {
				return true
			}
		}
		return false
	})

	// Bob takes the driver's seat; alice's attempt is refused (first request wins).
	bob.send(&pb.ClientMsg{Msg: &pb.ClientMsg_Claim{Claim: &pb.ClaimObject{Id: sp.Id, Seat: 0}}})
	oc := alice.expect("ownership", func(m *pb.ServerMsg) bool {
		return m.GetOwnership() != nil && m.GetOwnership().Id == sp.Id
	}).GetOwnership()
	if oc.Owner != bob.user || oc.Epoch != 2 || oc.LastState == nil {
		t.Fatalf("bob owns the jeep at epoch 2: %+v", oc)
	}
	alice.send(&pb.ClientMsg{Msg: &pb.ClientMsg_Claim{Claim: &pb.ClaimObject{Id: sp.Id, Seat: 0}}})
	alice.expect("seat taken", func(m *pb.ServerMsg) bool { return m.GetError() != nil && m.GetError().Code == "claim" })

	// Stale epoch from bob is dropped; current epoch is relayed.
	bob.udpSend(&pb.UdpPacket{Updates: []*pb.ObjectUpdate{{Id: sp.Id, Epoch: 1, State: &pb.ObjectState{Pos: &pb.Vec3{X: 1}}}}})
	alice.noUDP("stale epoch", func(p *pb.UdpPacket) bool {
		for _, u := range p.Updates {
			if u.Id == sp.Id && u.State.Pos.X == 1 {
				return true
			}
		}
		return false
	})
	bob.udpSend(&pb.UdpPacket{Updates: []*pb.ObjectUpdate{{Id: sp.Id, Epoch: 2, State: &pb.ObjectState{Pos: &pb.Vec3{X: 7}, Rot: &pb.Quat{W: 1}}}}})
	alice.expectUDP("bob drives", func(p *pb.UdpPacket) bool {
		return len(p.Updates) > 0 && p.Updates[0].Id == sp.Id && p.Updates[0].State.Pos.X == 7
	})

	// Bob disconnects: the jeep goes back to the server.
	bob.tcp.Close()
	alice.expect("jeep released", func(m *pb.ServerMsg) bool {
		o := m.GetOwnership()
		return o != nil && o.Id == sp.Id && o.Owner == 0 && o.Epoch == 3
	})
	alice.expect("bob left", func(m *pb.ServerMsg) bool { return m.GetPlayerLeft() != nil })

	// Progress persists across leaving and continuing the room.
	alice.send(&pb.ClientMsg{Msg: &pb.ClientMsg_SaveProgress{SaveProgress: &pb.SaveProgress{Progress: &pb.Progress{
		Cash: 1234, Xp: 50, BaseJson: `{"owned":["dropper_1"]}`,
	}}}})
	alice.send(&pb.ClientMsg{Msg: &pb.ClientMsg_LeaveRoom{LeaveRoom: &pb.LeaveRoom{}}})
	alice.expect("room_left", func(m *pb.ServerMsg) bool { return m.GetRoomLeft() != nil })
	time.Sleep(400 * time.Millisecond) // room unloads when idle
	alice.send(&pb.ClientMsg{Msg: &pb.ClientMsg_JoinRoom{JoinRoom: &pb.JoinRoom{RoomId: roomID}}})
	aj = alice.joined(alice.expect("room_joined again", isJoined))
	if aj.You.Progress.Cash != 1234 || aj.You.Progress.BaseJson != `{"owned":["dropper_1"]}` {
		t.Fatalf("progress restored: %+v", aj.You.Progress)
	}
	if len(aj.Players) != 1 || aj.Players[0].Username != "bob" || aj.Players[0].Online {
		t.Fatal("room remembers offline players")
	}

	// Archive: everyone leaves, room hidden from active list, host can restore it.
	alice.send(&pb.ClientMsg{Msg: &pb.ClientMsg_Admin{Admin: &pb.AdminCommand{Cmd: &pb.AdminCommand_Archive{Archive: true}}}})
	alice.expect("archived", func(m *pb.ServerMsg) bool { return m.GetRoomLeft() != nil })
	alice.send(&pb.ClientMsg{Msg: &pb.ClientMsg_ListRooms{ListRooms: &pb.ListRooms{}}})
	if l := alice.expect("list", func(m *pb.ServerMsg) bool { return m.GetRoomList() != nil }); len(l.GetRoomList().Rooms) != 0 {
		t.Fatal("archived room not in active list")
	}
	alice.send(&pb.ClientMsg{Msg: &pb.ClientMsg_JoinRoom{JoinRoom: &pb.JoinRoom{RoomId: roomID}}})
	alice.expect("join archived fails", func(m *pb.ServerMsg) bool { return m.GetError() != nil })
	alice.send(&pb.ClientMsg{Msg: &pb.ClientMsg_Admin{Admin: &pb.AdminCommand{Cmd: &pb.AdminCommand_UnarchiveRoom{UnarchiveRoom: roomID}}}})
	l := alice.expect("list after unarchive", func(m *pb.ServerMsg) bool { return m.GetRoomList() != nil }).GetRoomList()
	if len(l.Rooms) != 1 || l.Rooms[0].Archived {
		t.Fatal("room restored")
	}
	if r, _ := st.RoomByID(context.Background(), roomID); r.ID != roomID {
		t.Fatal("room still exists")
	}
}

func TestAdminPermissions(t *testing.T) {
	srv, _, stop := startServer(t)
	defer stop()
	host, guest := dial(t, srv), dial(t, srv)
	host.register("hosty")
	guest.register("guesty")
	host.send(&pb.ClientMsg{Msg: &pb.ClientMsg_CreateRoom{CreateRoom: &pb.CreateRoom{Name: "Admin"}}})
	hj := host.joined(host.expect("joined", isJoined))
	guest.send(&pb.ClientMsg{Msg: &pb.ClientMsg_JoinRoom{JoinRoom: &pb.JoinRoom{RoomId: hj.Room.Id}}})
	guest.joined(guest.expect("joined", isJoined))

	guest.send(&pb.ClientMsg{Msg: &pb.ClientMsg_Admin{Admin: &pb.AdminCommand{Cmd: &pb.AdminCommand_Kick{Kick: host.user}}}})
	guest.expect("not host", func(m *pb.ServerMsg) bool { return m.GetError() != nil && m.GetError().Code == "admin" })

	host.send(&pb.ClientMsg{Msg: &pb.ClientMsg_Admin{Admin: &pb.AdminCommand{Cmd: &pb.AdminCommand_GiveCash{GiveCash: &pb.GiveCash{UserId: guest.user, Amount: 500}}}}})
	if r := guest.expect("reward", func(m *pb.ServerMsg) bool { return m.GetReward() != nil }).GetReward(); r.Cash != 500 {
		t.Fatal("grant reaches guest")
	}

	host.send(&pb.ClientMsg{Msg: &pb.ClientMsg_Admin{Admin: &pb.AdminCommand{Cmd: &pb.AdminCommand_Ban{Ban: guest.user}}}})
	guest.expect("banned", func(m *pb.ServerMsg) bool { return m.GetRoomLeft() != nil })
	guest.send(&pb.ClientMsg{Msg: &pb.ClientMsg_JoinRoom{JoinRoom: &pb.JoinRoom{RoomId: hj.Room.Id}}})
	guest.expect("cannot rejoin", func(m *pb.ServerMsg) bool { return m.GetError() != nil && m.GetError().Code == "join_room" })

	host.send(&pb.ClientMsg{Msg: &pb.ClientMsg_ListEvents{ListEvents: &pb.ListEvents{}}})
	log := host.expect("log", func(m *pb.ServerMsg) bool { return m.GetEventLog() != nil }).GetEventLog()
	actions := map[string]bool{}
	for _, e := range log.Events {
		actions[e.Action] = true
	}
	if !actions["ban"] || !actions["give_cash"] || !actions["create"] {
		t.Fatalf("audit log: %v", actions)
	}
}
