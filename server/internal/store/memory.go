package store

import (
	"context"
	"sort"
	"strings"
	"sync"
	"time"
)

// Memory is an in-process Store used by tests and by `-memory` dev runs.
type Memory struct {
	mu       sync.Mutex
	nextUser uint64
	nextRoom uint64
	users    map[uint64]User
	sessions map[string]session
	rooms    map[uint64]*Room
	members  map[[2]uint64]Member
	players  map[[2]uint64]Player
	objects  map[uint64][]Object
	events   []Event
}

type session struct {
	user    uint64
	expires time.Time
}

func NewMemory() *Memory {
	return &Memory{
		users:    map[uint64]User{},
		sessions: map[string]session{},
		rooms:    map[uint64]*Room{},
		members:  map[[2]uint64]Member{},
		players:  map[[2]uint64]Player{},
		objects:  map[uint64][]Object{},
	}
}

func (m *Memory) CreateUser(_ context.Context, username, hash string) (uint64, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	for _, u := range m.users {
		if strings.EqualFold(u.Username, username) {
			return 0, ErrUsernameTaken
		}
	}
	m.nextUser++
	m.users[m.nextUser] = User{ID: m.nextUser, Username: username, PasswordHash: hash}
	return m.nextUser, nil
}

func (m *Memory) UserByName(_ context.Context, username string) (User, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	for _, u := range m.users {
		if strings.EqualFold(u.Username, username) {
			return u, nil
		}
	}
	return User{}, ErrNotFound
}

func (m *Memory) UserByID(_ context.Context, id uint64) (User, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	u, ok := m.users[id]
	if !ok {
		return User{}, ErrNotFound
	}
	return u, nil
}

func (m *Memory) CreateSession(_ context.Context, userID uint64, tokenHash []byte, expires time.Time) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.sessions[string(tokenHash)] = session{userID, expires}
	return nil
}

func (m *Memory) SessionUser(_ context.Context, tokenHash []byte) (uint64, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	s, ok := m.sessions[string(tokenHash)]
	if !ok || time.Now().After(s.expires) {
		return 0, ErrNotFound
	}
	return s.user, nil
}

func (m *Memory) roomCopy(r *Room) Room {
	c := *r
	c.HostName = m.users[r.HostID].Username
	c.Settings = append([]byte(nil), r.Settings...)
	return c
}

func (m *Memory) CreateRoom(_ context.Context, name string, hostID uint64, settings []byte) (Room, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.nextRoom++
	r := &Room{ID: m.nextRoom, Name: name, HostID: hostID, Settings: settings, NextObjectID: 1, CreatedAt: time.Now()}
	m.rooms[r.ID] = r
	m.members[[2]uint64{r.ID, hostID}] = Member{RoomID: r.ID, UserID: hostID}
	return m.roomCopy(r), nil
}

func (m *Memory) RoomByID(_ context.Context, id uint64) (Room, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	r, ok := m.rooms[id]
	if !ok {
		return Room{}, ErrNotFound
	}
	return m.roomCopy(r), nil
}

func (m *Memory) ListRooms(_ context.Context, userID uint64, includeArchived bool) ([]Room, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var out []Room
	for _, r := range m.rooms {
		if r.ArchivedAt != nil && !includeArchived {
			continue
		}
		c := m.roomCopy(r)
		mem, ok := m.members[[2]uint64{r.ID, userID}]
		c.Member = ok && !mem.Banned
		out = append(out, c)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID > out[j].ID })
	return out, nil
}

func (m *Memory) SetRoomArchived(_ context.Context, id uint64, archived bool) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	r, ok := m.rooms[id]
	if !ok {
		return ErrNotFound
	}
	if archived {
		if r.ArchivedAt == nil {
			now := time.Now()
			r.ArchivedAt = &now
		}
	} else {
		r.ArchivedAt = nil
	}
	return nil
}

func (m *Memory) SetRoomHost(_ context.Context, id, hostID uint64) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if r, ok := m.rooms[id]; ok {
		r.HostID = hostID
	}
	return nil
}

func (m *Memory) SetRoomSettings(_ context.Context, id uint64, settings []byte) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if r, ok := m.rooms[id]; ok {
		r.Settings = append([]byte(nil), settings...)
	}
	return nil
}

func (m *Memory) SetRoomNextObjectID(_ context.Context, id, next uint64) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if r, ok := m.rooms[id]; ok && next > r.NextObjectID {
		r.NextObjectID = next
	}
	return nil
}

func (m *Memory) Member(_ context.Context, roomID, userID uint64) (Member, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	mem, ok := m.members[[2]uint64{roomID, userID}]
	if !ok {
		return Member{}, ErrNotFound
	}
	return mem, nil
}

func (m *Memory) AddMember(_ context.Context, roomID, userID uint64) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	k := [2]uint64{roomID, userID}
	if _, ok := m.members[k]; !ok {
		m.members[k] = Member{RoomID: roomID, UserID: userID}
	}
	return nil
}

func (m *Memory) SetBanned(_ context.Context, roomID, userID uint64, banned bool) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.members[[2]uint64{roomID, userID}] = Member{RoomID: roomID, UserID: userID, Banned: banned}
	return nil
}

func (m *Memory) Players(_ context.Context, roomID uint64) ([]Player, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var out []Player
	for k, p := range m.players {
		if k[0] == roomID {
			p.Username = m.users[p.UserID].Username
			out = append(out, p)
		}
	}
	return out, nil
}

func (m *Memory) SavePlayer(_ context.Context, p Player) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.players[[2]uint64{p.RoomID, p.UserID}] = p
	return nil
}

func (m *Memory) Objects(_ context.Context, roomID uint64) ([]Object, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]Object(nil), m.objects[roomID]...), nil
}

func (m *Memory) ReplaceObjects(_ context.Context, roomID uint64, objs []Object) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.objects[roomID] = append([]Object(nil), objs...)
	return nil
}

func (m *Memory) LogEvent(_ context.Context, e Event) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	e.At = time.Now()
	m.events = append(m.events, e)
	return nil
}

func (m *Memory) Events(_ context.Context, roomID uint64, limit int) ([]Event, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var out []Event
	for i := len(m.events) - 1; i >= 0 && len(out) < limit; i-- {
		if m.events[i].RoomID == roomID {
			out = append(out, m.events[i])
		}
	}
	return out, nil
}
