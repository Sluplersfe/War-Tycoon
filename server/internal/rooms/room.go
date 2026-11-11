package rooms

import (
	"context"
	"encoding/json"
	"sync"
	"time"

	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"

	"frontline/internal/ownership"
	"frontline/internal/pb"
	"frontline/internal/store"
)

const (
	maxUpdatesPerPacket = 64
	maxShotsPerPacket   = 8
	serverUpdatesChunk  = 40
	maxBaseJSON         = 64 << 10
	maxVehiclesPerUser  = 6
)

// Player is the persisted per-room record of anyone who has ever joined.
type Player struct {
	store.Player
	Banned bool
	dirty  bool
}

// conn is an online player.
type conn struct {
	peer   Peer
	plot   int
	avatar uint64
	limits map[string]*window
}

type window struct {
	start time.Time
	n     int
}

// allow is a fixed-window rate limit: at most n events per period for key.
func (c *conn) allow(key string, n int, period time.Duration) bool {
	w := c.limits[key]
	now := time.Now()
	if w == nil {
		w = &window{start: now}
		c.limits[key] = w
	}
	if now.Sub(w.start) > period {
		w.start, w.n = now, 0
	}
	w.n++
	return w.n <= n
}

type Room struct {
	m  *Manager
	id uint64

	mu        sync.Mutex
	name      string
	hostID    uint64
	createdAt time.Time
	settings  *pb.RoomSettings
	players   map[uint64]*Player
	conns     map[uint64]*conn
	plots     []uint64
	reg       *ownership.Registry
	tick      uint32
	seq       uint32
	closed    bool
	emptyAt   time.Time

	saveMu sync.Mutex
	stop   chan struct{}
}

func newRoom(m *Manager, row store.Room, s *pb.RoomSettings) *Room {
	return &Room{
		m: m, id: row.ID, name: row.Name, hostID: row.HostID, createdAt: row.CreatedAt, settings: s,
		players: map[uint64]*Player{}, conns: map[uint64]*conn{},
		plots:   make([]uint64, m.world.Map.Plots.Count),
		reg:     ownership.NewRegistry(m.world, row.NextObjectID),
		emptyAt: time.Now(), stop: make(chan struct{}),
	}
}

func (r *Room) ID() uint64 { return r.id }

func (r *Room) OnlineCount() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.conns)
}

func errMsg(code, text string) *pb.ServerMsg {
	return &pb.ServerMsg{Msg: &pb.ServerMsg_Error{Error: &pb.Error{Code: code, Message: text}}}
}

// ---------- membership ----------

func (r *Room) join(ctx context.Context, p Peer) error {
	uid := p.UserID()
	mem, memErr := r.m.store.Member(ctx, r.id, uid)
	known := memErr == nil

	r.mu.Lock()
	defer r.mu.Unlock()
	if r.closed {
		return errClosed
	}
	if known && mem.Banned {
		return ErrBanned
	}
	if !known && r.settings.Locked && uid != r.hostID {
		return ErrLocked
	}
	if old := r.conns[uid]; old != nil {
		r.removeConn(old, "you joined from another connection")
	}
	if len(r.conns) >= int(r.settings.MaxPlayers) || len(r.conns) >= len(r.plots) {
		return ErrFull
	}
	if !known {
		if err := r.m.store.AddMember(ctx, r.id, uid); err != nil {
			return err
		}
	}

	pl := r.players[uid]
	if pl == nil {
		pl = &Player{Player: store.Player{
			RoomID: r.id, UserID: uid, Cash: r.settings.StartingCash, BaseState: "{}", Team: r.smallestTeam(),
		}, dirty: true}
		r.players[uid] = pl
	}
	pl.Username = p.Username()
	if int(pl.Team) >= len(r.settings.Teams) {
		pl.Team = r.smallestTeam()
	}

	plot := 0
	for i, owner := range r.plots {
		if owner == 0 {
			plot = i
			break
		}
	}
	r.plots[plot] = uid
	spawn := r.m.world.PlotSpawn(plot)
	_, _, yaw := r.m.world.Plot(plot)
	avatar := r.reg.Spawn(pb.ObjectKind_OBJECT_KIND_PLAYER, "soldier", uid, uid, &pb.ObjectState{
		Pos: &pb.Vec3{X: spawn[0], Y: spawn[1], Z: spawn[2]}, Rot: yawQuat(yaw), Health: 100,
	})
	c := &conn{peer: p, plot: plot, avatar: avatar.ID, limits: map[string]*window{}}
	r.conns[uid] = c

	info := r.playerInfo(pl)
	others := make([]*pb.PlayerInfo, 0, len(r.players))
	for _, op := range r.players {
		if op.UserID != uid {
			others = append(others, r.playerInfo(op))
		}
	}
	p.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_RoomJoined{RoomJoined: &pb.RoomJoined{
		Room: r.summary(), Settings: r.settings, ConnId: p.ConnID(), UdpKey: p.UDPKey(),
		You: info, Players: others, Objects: r.reg.Snapshot(), TickRate: uint32(r.m.cfg.TickRate),
	}}})
	r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_PlayerJoined{PlayerJoined: &pb.PlayerJoined{Player: info}}}, uid)
	r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_ObjectSpawned{ObjectSpawned: &pb.ObjectSpawned{Object: avatar.Info()}}}, uid)
	r.systemChat(pl.Username + " joined")
	r.m.log.Info("join", "room", r.id, "user", pl.Username, "plot", plot)
	return nil
}

func (r *Room) smallestTeam() uint32 {
	counts := make([]int, len(r.settings.Teams))
	for uid := range r.conns {
		if pl := r.players[uid]; pl != nil && int(pl.Team) < len(counts) {
			counts[pl.Team]++
		}
	}
	best := 0
	for i, n := range counts {
		if n < counts[best] {
			best = i
		}
	}
	return uint32(best)
}

// Leave is called when the client leaves or disconnects.
func (r *Room) Leave(p Peer) {
	r.mu.Lock()
	c := r.conns[p.UserID()]
	if c != nil && c.peer == p {
		r.removeConn(c, "")
	}
	r.mu.Unlock()
	go r.save(context.Background())
}

// removeConn drops an online player: their character is removed, their
// vehicles go to the server and their plot is freed. Caller holds r.mu.
func (r *Room) removeConn(c *conn, reason string) {
	uid := c.peer.UserID()
	delete(r.conns, uid)
	if r.plots[c.plot] == uid {
		r.plots[c.plot] = 0
	}
	changes, despawned := r.reg.ReleaseAll(uid)
	for _, id := range despawned {
		r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_ObjectDespawned{ObjectDespawned: &pb.ObjectDespawned{Id: id}}}, 0)
	}
	r.broadcastChanges(changes)
	r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_PlayerLeft{PlayerLeft: &pb.PlayerLeft{UserId: uid}}}, 0)
	if pl := r.players[uid]; pl != nil {
		pl.dirty = true
		r.systemChat(pl.Username + " left")
	}
	if len(r.conns) == 0 {
		r.emptyAt = time.Now()
	}
	if reason != "" {
		c.peer.LeftRoom(r, reason)
	}
}

// ---------- helpers ----------

func (r *Room) summary() *pb.RoomSummary {
	host := ""
	if pl := r.players[r.hostID]; pl != nil {
		host = pl.Username
	}
	return &pb.RoomSummary{
		Id: r.id, Name: r.name, HostId: r.hostID, HostName: host, Online: uint32(len(r.conns)),
		MaxPlayers: r.settings.MaxPlayers, Member: true, CreatedAt: r.createdAt.UnixMilli(), Live: true,
	}
}

func (r *Room) progress(pl *Player) *pb.Progress {
	return &pb.Progress{
		Cash: pl.Cash, Xp: pl.XP, Rank: pl.Rank, Rebirths: pl.Rebirths, BaseJson: pl.BaseState,
		Kills: pl.Kills, Deaths: pl.Deaths,
	}
}

func (r *Room) playerInfo(pl *Player) *pb.PlayerInfo {
	info := &pb.PlayerInfo{
		UserId: pl.UserID, Username: pl.Username, Team: pl.Team, Host: pl.UserID == r.hostID,
		Progress: r.progress(pl), Banned: pl.Banned,
	}
	if c := r.conns[pl.UserID]; c != nil {
		info.Online = true
		info.Plot = uint32(c.plot)
		info.AvatarId = c.avatar
	}
	return info
}

func (r *Room) broadcast(msg *pb.ServerMsg, except uint64) {
	for uid, c := range r.conns {
		if uid != except {
			c.peer.Send(msg)
		}
	}
}

func (r *Room) broadcastChanges(changes []ownership.Change) {
	for _, ch := range changes {
		r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_Ownership{Ownership: ch.Msg()}}, 0)
	}
}

func (r *Room) playerUpdated(pl *Player) {
	r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_PlayerUpdated{PlayerUpdated: &pb.PlayerUpdated{Player: r.playerInfo(pl)}}}, 0)
}

func (r *Room) systemChat(text string) {
	r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_Chat{Chat: &pb.ChatMsg{Text: text, System: true}}}, 0)
}

func (r *Room) logEvent(actor uint64, action string, target uint64, detail any) {
	d := ""
	if detail != nil {
		if b, err := json.Marshal(detail); err == nil {
			d = string(b)
		}
	}
	if err := r.m.store.LogEvent(context.Background(), store.Event{
		RoomID: r.id, ActorID: actor, Action: action, TargetID: target, Detail: d,
	}); err != nil {
		r.m.log.Error("log event", "room", r.id, "err", err)
	}
}

func yawQuat(yaw float32) *pb.Quat {
	q := &pb.Quat{}
	s, c := sincos(yaw / 2)
	q.Y, q.W = s, c
	return q
}

// ---------- tick loop and persistence ----------

func (r *Room) run() {
	defer r.m.wg.Done()
	rate := max(1, r.m.cfg.TickRate)
	dt := float32(1) / float32(rate)
	ticker := time.NewTicker(time.Second / time.Duration(rate))
	defer ticker.Stop()
	lastSave := time.Now()
	for {
		select {
		case <-r.stop:
			return
		case <-ticker.C:
		}
		r.step(dt)
		if time.Since(lastSave) >= r.m.cfg.Autosave {
			lastSave = time.Now()
			r.save(context.Background())
		}
		if r.tryUnload() {
			return
		}
	}
}

func (r *Room) step(dt float32) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.closed {
		return
	}
	r.tick++
	ttl := r.m.cfg.AbandonTTL
	if r.settings.PersistVehicles {
		ttl = 0
	}
	moved, gone := r.reg.Step(dt, ttl)
	for _, id := range gone {
		r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_ObjectDespawned{ObjectDespawned: &pb.ObjectDespawned{Id: id}}}, 0)
	}
	if len(r.conns) == 0 {
		return
	}
	for i := 0; i < len(moved); i += serverUpdatesChunk {
		pkt := &pb.UdpPacket{Tick: r.tick}
		for _, o := range moved[i:min(i+serverUpdatesChunk, len(moved))] {
			pkt.Updates = append(pkt.Updates, &pb.ObjectUpdate{Id: o.ID, Epoch: o.Epoch, State: o.State})
			o.Dirty = false
		}
		r.sendUDPAll(pkt, 0)
	}
}

func (r *Room) sendUDPAll(pkt *pb.UdpPacket, except uint64) {
	r.seq++
	pkt.Seq = r.seq
	b, err := proto.Marshal(pkt)
	if err != nil {
		return
	}
	for uid, c := range r.conns {
		if uid != except {
			c.peer.SendUDP(b)
		}
	}
}

func (r *Room) tryUnload() bool {
	r.m.mu.Lock()
	r.mu.Lock()
	idle := !r.closed && len(r.conns) == 0 && time.Since(r.emptyAt) >= r.m.cfg.IdleUnload
	if idle {
		r.closed = true
		r.m.detach(r)
	}
	r.mu.Unlock()
	r.m.mu.Unlock()
	if idle {
		r.save(context.Background())
		r.m.log.Info("room unloaded", "room", r.id)
	}
	return idle
}

// close kicks everyone, saves and stops the room. With archive set, the room
// is also marked archived.
func (r *Room) close(ctx context.Context, reason string, archive bool) {
	r.m.mu.Lock()
	r.mu.Lock()
	if r.closed {
		r.mu.Unlock()
		r.m.mu.Unlock()
		return
	}
	for _, c := range r.conns {
		r.removeConn(c, reason)
	}
	r.closed = true
	r.m.detach(r)
	r.mu.Unlock()
	r.m.mu.Unlock()

	r.save(ctx)
	if archive {
		if err := r.m.store.SetRoomArchived(ctx, r.id, true); err != nil {
			r.m.log.Error("archive", "room", r.id, "err", err)
		}
	}
	close(r.stop)
}

type snapshot struct {
	players []store.Player
	objects []store.Object
	persist bool
	nextID  uint64
}

func (r *Room) snapshot() snapshot {
	r.mu.Lock()
	defer r.mu.Unlock()
	s := snapshot{persist: r.settings.PersistVehicles, nextID: r.reg.NextID}
	for _, pl := range r.players {
		if pl.dirty {
			s.players = append(s.players, pl.Player)
			pl.dirty = false
		}
	}
	if s.persist {
		for _, o := range r.reg.Objects {
			if o.Kind == pb.ObjectKind_OBJECT_KIND_PLAYER {
				continue
			}
			b, err := protojson.Marshal(o.State)
			if err != nil {
				continue
			}
			s.objects = append(s.objects, store.Object{
				RoomID: r.id, ObjectID: o.ID, Kind: int32(o.Kind), Def: o.Def, State: b, SpawnedBy: o.SpawnedBy,
			})
		}
	}
	return s
}

func (r *Room) save(ctx context.Context) {
	r.saveMu.Lock()
	defer r.saveMu.Unlock()
	s := r.snapshot()
	for _, p := range s.players {
		if err := r.m.store.SavePlayer(ctx, p); err != nil {
			r.m.log.Error("save player", "room", r.id, "user", p.UserID, "err", err)
			r.mu.Lock()
			if pl := r.players[p.UserID]; pl != nil {
				pl.dirty = true
			}
			r.mu.Unlock()
		}
	}
	if s.persist {
		if err := r.m.store.ReplaceObjects(ctx, r.id, s.objects); err != nil {
			r.m.log.Error("save objects", "room", r.id, "err", err)
		}
	}
	if err := r.m.store.SetRoomNextObjectID(ctx, r.id, s.nextID); err != nil {
		r.m.log.Error("save next id", "room", r.id, "err", err)
	}
}
