// Package rooms runs live rooms. A room is a save file holding every player who
// has ever joined it; it is loaded when someone joins ("continue"), autosaved,
// unloaded when empty, and can be archived but never deleted.
package rooms

import (
	"context"
	"errors"
	"log/slog"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"google.golang.org/protobuf/encoding/protojson"

	"frontline/internal/admin"
	"frontline/internal/pb"
	"frontline/internal/store"
	"frontline/internal/world"
)

var (
	ErrRoomNotFound = errors.New("room not found")
	ErrArchived     = errors.New("room is archived")
	ErrBanned       = errors.New("you are banned from this room")
	ErrLocked       = errors.New("room is locked to existing members")
	ErrFull         = errors.New("room is full")
	ErrBadName      = errors.New("room name must be 1-40 characters")
	errClosed       = errors.New("room closed")
)

// Peer is a connected, authenticated client as seen by a room.
type Peer interface {
	UserID() uint64
	Username() string
	ConnID() uint32
	UDPKey() uint64
	Send(*pb.ServerMsg)
	SendUDP([]byte)
	// LeftRoom tells the connection it was removed from r (kick, archive,
	// shutdown). It must not call back into the room.
	LeftRoom(r *Room, reason string)
}

type Config struct {
	TickRate   int
	Autosave   time.Duration
	IdleUnload time.Duration
	AbandonTTL time.Duration
}

func DefaultConfig() Config {
	return Config{TickRate: 20, Autosave: 30 * time.Second, IdleUnload: 60 * time.Second, AbandonTTL: 5 * time.Minute}
}

type Manager struct {
	store store.Store
	world *world.World
	cfg   Config
	log   *slog.Logger

	mu   sync.Mutex
	live map[uint64]*Room
	wg   sync.WaitGroup
}

func NewManager(st store.Store, w *world.World, cfg Config, log *slog.Logger) *Manager {
	return &Manager{store: st, world: w, cfg: cfg, log: log, live: map[uint64]*Room{}}
}

func (m *Manager) World() *world.World { return m.world }

func (m *Manager) Create(ctx context.Context, hostID uint64, name string, s *pb.RoomSettings) (*pb.RoomSummary, error) {
	name = strings.TrimSpace(name)
	if name == "" || utf8.RuneCountInString(name) > 40 {
		return nil, ErrBadName
	}
	s, err := admin.Normalize(s, m.world.Map.Plots.Count)
	if err != nil {
		return nil, err
	}
	b, err := protojson.Marshal(s)
	if err != nil {
		return nil, err
	}
	row, err := m.store.CreateRoom(ctx, name, hostID, b)
	if err != nil {
		return nil, err
	}
	m.store.LogEvent(ctx, store.Event{RoomID: row.ID, ActorID: hostID, Action: "create"})
	return summaryOf(row, s, 0, false), nil
}

func summaryOf(row store.Room, s *pb.RoomSettings, online int, live bool) *pb.RoomSummary {
	sum := &pb.RoomSummary{
		Id: row.ID, Name: row.Name, HostId: row.HostID, HostName: row.HostName,
		Online: uint32(online), Archived: row.ArchivedAt != nil, Member: row.Member,
		CreatedAt: row.CreatedAt.UnixMilli(), Live: live,
	}
	if s != nil {
		sum.MaxPlayers = s.MaxPlayers
	}
	return sum
}

func (m *Manager) List(ctx context.Context, userID uint64, includeArchived bool) (*pb.RoomList, error) {
	rows, err := m.store.ListRooms(ctx, userID, includeArchived)
	if err != nil {
		return nil, err
	}
	out := &pb.RoomList{}
	for _, row := range rows {
		s := &pb.RoomSettings{}
		_ = protojson.Unmarshal(row.Settings, s)
		online := 0
		m.mu.Lock()
		r := m.live[row.ID]
		m.mu.Unlock()
		if r != nil {
			online = r.OnlineCount()
		}
		out.Rooms = append(out.Rooms, summaryOf(row, s, online, r != nil))
	}
	return out, nil
}

// Join loads the room if needed (continuing it from its save) and adds p.
func (m *Manager) Join(ctx context.Context, p Peer, roomID uint64) (*Room, error) {
	for attempt := 0; attempt < 3; attempt++ {
		r, err := m.get(ctx, roomID)
		if err != nil {
			return nil, err
		}
		if err := r.join(ctx, p); err != errClosed {
			if err != nil {
				return nil, err
			}
			return r, nil
		}
	}
	return nil, errors.New("room is busy, try again")
}

func (m *Manager) get(ctx context.Context, id uint64) (*Room, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if r := m.live[id]; r != nil {
		return r, nil
	}
	r, err := m.load(ctx, id)
	if err != nil {
		return nil, err
	}
	m.live[id] = r
	m.wg.Add(1)
	go r.run()
	return r, nil
}

func (m *Manager) load(ctx context.Context, id uint64) (*Room, error) {
	row, err := m.store.RoomByID(ctx, id)
	if errors.Is(err, store.ErrNotFound) {
		return nil, ErrRoomNotFound
	}
	if err != nil {
		return nil, err
	}
	if row.ArchivedAt != nil {
		return nil, ErrArchived
	}
	s := &pb.RoomSettings{}
	if err := protojson.Unmarshal(row.Settings, s); err != nil {
		m.log.Warn("bad room settings, using defaults", "room", id, "err", err)
	}
	if s, err = admin.Normalize(s, m.world.Map.Plots.Count); err != nil {
		s = admin.DefaultSettings()
	}
	r := newRoom(m, row, s)
	players, err := m.store.Players(ctx, id)
	if err != nil {
		return nil, err
	}
	for _, p := range players {
		pl := &Player{Player: p}
		if mem, err := m.store.Member(ctx, id, p.UserID); err == nil {
			pl.Banned = mem.Banned
		}
		r.players[p.UserID] = pl
	}
	if s.PersistVehicles {
		objs, err := m.store.Objects(ctx, id)
		if err != nil {
			return nil, err
		}
		for _, o := range objs {
			st := &pb.ObjectState{}
			if err := protojson.Unmarshal(o.State, st); err != nil {
				continue
			}
			r.reg.Restore(o.ObjectID, pb.ObjectKind(o.Kind), o.Def, o.SpawnedBy, st)
		}
	}
	m.log.Info("room loaded", "room", id, "name", row.Name, "players", len(players))
	return r, nil
}

// Unarchive is done from the lobby by the room's host.
func (m *Manager) Unarchive(ctx context.Context, userID, roomID uint64) error {
	row, err := m.store.RoomByID(ctx, roomID)
	if errors.Is(err, store.ErrNotFound) {
		return ErrRoomNotFound
	}
	if err != nil {
		return err
	}
	if row.HostID != userID {
		return admin.ErrNotHost
	}
	if err := m.store.SetRoomArchived(ctx, roomID, false); err != nil {
		return err
	}
	return m.store.LogEvent(ctx, store.Event{RoomID: roomID, ActorID: userID, Action: "unarchive"})
}

// detach removes r from the live set if it is still there. Caller holds m.mu.
func (m *Manager) detach(r *Room) {
	if m.live[r.id] == r {
		delete(m.live, r.id)
	}
}

// Shutdown saves and closes every live room.
func (m *Manager) Shutdown(ctx context.Context) {
	m.mu.Lock()
	rooms := make([]*Room, 0, len(m.live))
	for _, r := range m.live {
		rooms = append(rooms, r)
	}
	m.mu.Unlock()
	for _, r := range rooms {
		r.close(ctx, "server is restarting", false)
	}
	m.wg.Wait()
}

