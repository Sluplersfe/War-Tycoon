package store

import (
	"context"
	"os"
	"testing"
	"time"
)

// Runs only when FF_TEST_DATABASE_URL points at a disposable Postgres, e.g.
//   docker run --rm -d -p 55432:5432 -e POSTGRES_PASSWORD=test postgres:17-alpine
//   FF_TEST_DATABASE_URL=postgres://postgres:test@localhost:55432/postgres?sslmode=disable go test ./internal/store
func TestPostgres(t *testing.T) {
	url := os.Getenv("FF_TEST_DATABASE_URL")
	if url == "" {
		t.Skip("FF_TEST_DATABASE_URL not set")
	}
	ctx := context.Background()
	pg, err := OpenPostgres(ctx, url)
	if err != nil {
		t.Fatal(err)
	}
	defer pg.Close()
	testStore(t, pg)
}

func TestMemory(t *testing.T) { testStore(t, NewMemory()) }

func testStore(t *testing.T, s Store) {
	ctx := context.Background()
	suffix := time.Now().Format("150405.000")
	name := "u" + suffix[:6] + suffix[7:]
	id, err := s.CreateUser(ctx, name, "hash")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.CreateUser(ctx, name, "x"); err != ErrUsernameTaken {
		t.Fatalf("duplicate username: %v", err)
	}
	if u, err := s.UserByName(ctx, name); err != nil || u.ID != id {
		t.Fatalf("by name: %v %v", u, err)
	}
	if err := s.CreateSession(ctx, id, []byte("tok"+suffix), time.Now().Add(time.Hour)); err != nil {
		t.Fatal(err)
	}
	if uid, err := s.SessionUser(ctx, []byte("tok"+suffix)); err != nil || uid != id {
		t.Fatalf("session: %v %v", uid, err)
	}

	room, err := s.CreateRoom(ctx, "Room "+suffix, id, []byte(`{"maxPlayers":8}`))
	if err != nil {
		t.Fatal(err)
	}
	if room.HostName != name {
		t.Fatalf("host name: %q", room.HostName)
	}
	if m, err := s.Member(ctx, room.ID, id); err != nil || m.Banned {
		t.Fatal("host is a member")
	}
	if err := s.SavePlayer(ctx, Player{RoomID: room.ID, UserID: id, Cash: 99, BaseState: `{"a":1}`}); err != nil {
		t.Fatal(err)
	}
	ps, err := s.Players(ctx, room.ID)
	if err != nil || len(ps) != 1 || ps[0].Cash != 99 || ps[0].Username != name {
		t.Fatalf("players: %+v %v", ps, err)
	}
	if err := s.SetRoomArchived(ctx, room.ID, true); err != nil {
		t.Fatal(err)
	}
	rooms, _ := s.ListRooms(ctx, id, false)
	for _, r := range rooms {
		if r.ID == room.ID {
			t.Fatal("archived room hidden")
		}
	}
	rooms, _ = s.ListRooms(ctx, id, true)
	found := false
	for _, r := range rooms {
		if r.ID == room.ID && r.ArchivedAt != nil && r.Member {
			found = true
		}
	}
	if !found {
		t.Fatal("archived room listed with include_archived")
	}
	if err := s.LogEvent(ctx, Event{RoomID: room.ID, ActorID: id, Action: "kick", TargetID: 5}); err != nil {
		t.Fatal(err)
	}
	evs, err := s.Events(ctx, room.ID, 10)
	if err != nil || len(evs) != 1 || evs[0].Action != "kick" || evs[0].TargetID != 5 {
		t.Fatalf("events: %+v %v", evs, err)
	}
	if err := s.ReplaceObjects(ctx, room.ID, []Object{{RoomID: room.ID, ObjectID: 3, Kind: 2, Def: "jeep", State: []byte(`{}`)}}); err != nil {
		t.Fatal(err)
	}
	if objs, _ := s.Objects(ctx, room.ID); len(objs) != 1 || objs[0].Def != "jeep" {
		t.Fatal("objects")
	}
}
