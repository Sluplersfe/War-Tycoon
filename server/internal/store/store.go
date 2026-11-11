// Package store persists users, sessions and rooms. Rooms are never deleted:
// there is intentionally no delete operation for rooms or their players.
package store

import (
	"context"
	"errors"
	"time"
)

var (
	ErrNotFound      = errors.New("not found")
	ErrUsernameTaken = errors.New("username taken")
)

type User struct {
	ID           uint64
	Username     string
	PasswordHash string
}

type Room struct {
	ID           uint64
	Name         string
	HostID       uint64
	HostName     string
	Settings     []byte // protojson RoomSettings
	NextObjectID uint64
	CreatedAt    time.Time
	ArchivedAt   *time.Time
	Member       bool // filled by ListRooms for the requesting user
}

type Member struct {
	RoomID uint64
	UserID uint64
	Banned bool
}

type Player struct {
	RoomID    uint64
	UserID    uint64
	Username  string
	Team      uint32
	Cash      int64
	XP        int64
	Rank      uint32
	Rebirths  uint32
	Kills     int64
	Deaths    int64
	BaseState string // JSON
}

type Object struct {
	RoomID    uint64
	ObjectID  uint64
	Kind      int32
	Def       string
	State     []byte // protojson ObjectState
	SpawnedBy uint64
}

type Event struct {
	At       time.Time
	RoomID   uint64
	ActorID  uint64
	Action   string
	TargetID uint64
	Detail   string // JSON
}

type Store interface {
	CreateUser(ctx context.Context, username, passwordHash string) (uint64, error)
	UserByName(ctx context.Context, username string) (User, error)
	UserByID(ctx context.Context, id uint64) (User, error)

	CreateSession(ctx context.Context, userID uint64, tokenHash []byte, expires time.Time) error
	SessionUser(ctx context.Context, tokenHash []byte) (uint64, error)

	CreateRoom(ctx context.Context, name string, hostID uint64, settings []byte) (Room, error)
	RoomByID(ctx context.Context, id uint64) (Room, error)
	ListRooms(ctx context.Context, userID uint64, includeArchived bool) ([]Room, error)
	SetRoomArchived(ctx context.Context, id uint64, archived bool) error
	SetRoomHost(ctx context.Context, id, hostID uint64) error
	SetRoomSettings(ctx context.Context, id uint64, settings []byte) error
	SetRoomNextObjectID(ctx context.Context, id, next uint64) error

	Member(ctx context.Context, roomID, userID uint64) (Member, error)
	AddMember(ctx context.Context, roomID, userID uint64) error
	SetBanned(ctx context.Context, roomID, userID uint64, banned bool) error

	Players(ctx context.Context, roomID uint64) ([]Player, error)
	SavePlayer(ctx context.Context, p Player) error

	Objects(ctx context.Context, roomID uint64) ([]Object, error)
	ReplaceObjects(ctx context.Context, roomID uint64, objs []Object) error

	LogEvent(ctx context.Context, e Event) error
	Events(ctx context.Context, roomID uint64, limit int) ([]Event, error)
}
