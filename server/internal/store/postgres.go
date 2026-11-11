package store

import (
	"context"
	"database/sql"
	"embed"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/jackc/pgx/v5/stdlib"
	"github.com/pressly/goose/v3"
)

//go:embed migrations/*.sql
var migrations embed.FS

type Postgres struct {
	pool *pgxpool.Pool
}

// OpenPostgres connects and runs migrations, retrying while the database starts.
func OpenPostgres(ctx context.Context, url string) (*Postgres, error) {
	var pool *pgxpool.Pool
	var err error
	for attempt := 0; attempt < 30; attempt++ {
		pool, err = pgxpool.New(ctx, url)
		if err == nil {
			if err = pool.Ping(ctx); err == nil {
				break
			}
			pool.Close()
		}
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(time.Second):
		}
	}
	if err != nil {
		return nil, fmt.Errorf("connect postgres: %w", err)
	}
	if err := migrate(pool); err != nil {
		pool.Close()
		return nil, err
	}
	return &Postgres{pool: pool}, nil
}

func migrate(pool *pgxpool.Pool) error {
	db := stdlib.OpenDBFromPool(pool)
	defer db.Close()
	goose.SetBaseFS(migrations)
	goose.SetLogger(goose.NopLogger())
	if err := goose.SetDialect("postgres"); err != nil {
		return err
	}
	if err := goose.Up(db, "migrations"); err != nil {
		return fmt.Errorf("migrate: %w", err)
	}
	return nil
}

func (p *Postgres) Close() { p.pool.Close() }

func notFound(err error) error {
	if errors.Is(err, pgx.ErrNoRows) || errors.Is(err, sql.ErrNoRows) {
		return ErrNotFound
	}
	return err
}

func (p *Postgres) CreateUser(ctx context.Context, username, hash string) (uint64, error) {
	var id uint64
	err := p.pool.QueryRow(ctx,
		`INSERT INTO users (username, username_lower, password_hash) VALUES ($1, $2, $3) RETURNING id`,
		username, strings.ToLower(username), hash).Scan(&id)
	var pgErr *pgconn.PgError
	if errors.As(err, &pgErr) && pgErr.Code == "23505" {
		return 0, ErrUsernameTaken
	}
	return id, err
}

func (p *Postgres) UserByName(ctx context.Context, username string) (User, error) {
	var u User
	err := p.pool.QueryRow(ctx,
		`SELECT id, username, password_hash FROM users WHERE username_lower = $1`,
		strings.ToLower(username)).Scan(&u.ID, &u.Username, &u.PasswordHash)
	return u, notFound(err)
}

func (p *Postgres) UserByID(ctx context.Context, id uint64) (User, error) {
	var u User
	err := p.pool.QueryRow(ctx,
		`SELECT id, username, password_hash FROM users WHERE id = $1`, id).
		Scan(&u.ID, &u.Username, &u.PasswordHash)
	return u, notFound(err)
}

func (p *Postgres) CreateSession(ctx context.Context, userID uint64, tokenHash []byte, expires time.Time) error {
	_, err := p.pool.Exec(ctx,
		`INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)`,
		tokenHash, userID, expires)
	return err
}

func (p *Postgres) SessionUser(ctx context.Context, tokenHash []byte) (uint64, error) {
	var id uint64
	err := p.pool.QueryRow(ctx,
		`SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > now()`, tokenHash).Scan(&id)
	return id, notFound(err)
}

const roomCols = `r.id, r.name, r.host_id, u.username, r.settings, r.next_object_id, r.created_at, r.archived_at`

func scanRoom(row pgx.Row, extra ...any) (Room, error) {
	var r Room
	dest := append([]any{&r.ID, &r.Name, &r.HostID, &r.HostName, &r.Settings, &r.NextObjectID, &r.CreatedAt, &r.ArchivedAt}, extra...)
	err := row.Scan(dest...)
	return r, notFound(err)
}

func (p *Postgres) CreateRoom(ctx context.Context, name string, hostID uint64, settings []byte) (Room, error) {
	tx, err := p.pool.Begin(ctx)
	if err != nil {
		return Room{}, err
	}
	defer tx.Rollback(ctx)
	var id uint64
	if err := tx.QueryRow(ctx,
		`INSERT INTO rooms (name, host_id, settings) VALUES ($1, $2, $3) RETURNING id`,
		name, hostID, settings).Scan(&id); err != nil {
		return Room{}, err
	}
	if _, err := tx.Exec(ctx,
		`INSERT INTO room_members (room_id, user_id) VALUES ($1, $2)`, id, hostID); err != nil {
		return Room{}, err
	}
	if err := tx.Commit(ctx); err != nil {
		return Room{}, err
	}
	return p.RoomByID(ctx, id)
}

func (p *Postgres) RoomByID(ctx context.Context, id uint64) (Room, error) {
	return scanRoom(p.pool.QueryRow(ctx,
		`SELECT `+roomCols+` FROM rooms r JOIN users u ON u.id = r.host_id WHERE r.id = $1`, id))
}

func (p *Postgres) ListRooms(ctx context.Context, userID uint64, includeArchived bool) ([]Room, error) {
	rows, err := p.pool.Query(ctx, `
		SELECT `+roomCols+`, (m.user_id IS NOT NULL AND NOT m.banned)
		FROM rooms r
		JOIN users u ON u.id = r.host_id
		LEFT JOIN room_members m ON m.room_id = r.id AND m.user_id = $1
		WHERE ($2 OR r.archived_at IS NULL)
		ORDER BY r.archived_at IS NOT NULL, r.created_at DESC
		LIMIT 500`, userID, includeArchived)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Room
	for rows.Next() {
		var member bool
		r, err := scanRoom(rows, &member)
		if err != nil {
			return nil, err
		}
		r.Member = member
		out = append(out, r)
	}
	return out, rows.Err()
}

func (p *Postgres) SetRoomArchived(ctx context.Context, id uint64, archived bool) error {
	q := `UPDATE rooms SET archived_at = NULL WHERE id = $1`
	if archived {
		q = `UPDATE rooms SET archived_at = now() WHERE id = $1 AND archived_at IS NULL`
	}
	_, err := p.pool.Exec(ctx, q, id)
	return err
}

func (p *Postgres) SetRoomHost(ctx context.Context, id, hostID uint64) error {
	_, err := p.pool.Exec(ctx, `UPDATE rooms SET host_id = $2 WHERE id = $1`, id, hostID)
	return err
}

func (p *Postgres) SetRoomSettings(ctx context.Context, id uint64, settings []byte) error {
	_, err := p.pool.Exec(ctx, `UPDATE rooms SET settings = $2 WHERE id = $1`, id, settings)
	return err
}

func (p *Postgres) SetRoomNextObjectID(ctx context.Context, id, next uint64) error {
	_, err := p.pool.Exec(ctx, `UPDATE rooms SET next_object_id = GREATEST(next_object_id, $2) WHERE id = $1`, id, next)
	return err
}

func (p *Postgres) Member(ctx context.Context, roomID, userID uint64) (Member, error) {
	m := Member{RoomID: roomID, UserID: userID}
	err := p.pool.QueryRow(ctx,
		`SELECT banned FROM room_members WHERE room_id = $1 AND user_id = $2`, roomID, userID).Scan(&m.Banned)
	return m, notFound(err)
}

func (p *Postgres) AddMember(ctx context.Context, roomID, userID uint64) error {
	_, err := p.pool.Exec(ctx,
		`INSERT INTO room_members (room_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING`, roomID, userID)
	return err
}

func (p *Postgres) SetBanned(ctx context.Context, roomID, userID uint64, banned bool) error {
	_, err := p.pool.Exec(ctx, `
		INSERT INTO room_members (room_id, user_id, banned) VALUES ($1, $2, $3)
		ON CONFLICT (room_id, user_id) DO UPDATE SET banned = EXCLUDED.banned`, roomID, userID, banned)
	return err
}

func (p *Postgres) Players(ctx context.Context, roomID uint64) ([]Player, error) {
	rows, err := p.pool.Query(ctx, `
		SELECT p.room_id, p.user_id, u.username, p.team, p.cash, p.xp, p.rank, p.rebirths,
		       p.kills, p.deaths, p.base_state::text
		FROM room_players p JOIN users u ON u.id = p.user_id
		WHERE p.room_id = $1`, roomID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Player
	for rows.Next() {
		var pl Player
		if err := rows.Scan(&pl.RoomID, &pl.UserID, &pl.Username, &pl.Team, &pl.Cash, &pl.XP,
			&pl.Rank, &pl.Rebirths, &pl.Kills, &pl.Deaths, &pl.BaseState); err != nil {
			return nil, err
		}
		out = append(out, pl)
	}
	return out, rows.Err()
}

func (p *Postgres) SavePlayer(ctx context.Context, pl Player) error {
	base := pl.BaseState
	if base == "" {
		base = "{}"
	}
	_, err := p.pool.Exec(ctx, `
		INSERT INTO room_players (room_id, user_id, team, cash, xp, rank, rebirths, kills, deaths, base_state, updated_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::jsonb, now())
		ON CONFLICT (room_id, user_id) DO UPDATE SET
			team = EXCLUDED.team, cash = EXCLUDED.cash, xp = EXCLUDED.xp, rank = EXCLUDED.rank,
			rebirths = EXCLUDED.rebirths, kills = EXCLUDED.kills, deaths = EXCLUDED.deaths,
			base_state = EXCLUDED.base_state, updated_at = now()`,
		pl.RoomID, pl.UserID, pl.Team, pl.Cash, pl.XP, pl.Rank, pl.Rebirths, pl.Kills, pl.Deaths, base)
	return err
}

func (p *Postgres) Objects(ctx context.Context, roomID uint64) ([]Object, error) {
	rows, err := p.pool.Query(ctx,
		`SELECT room_id, object_id, kind, def, state, spawned_by FROM room_objects WHERE room_id = $1`, roomID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Object
	for rows.Next() {
		var o Object
		if err := rows.Scan(&o.RoomID, &o.ObjectID, &o.Kind, &o.Def, &o.State, &o.SpawnedBy); err != nil {
			return nil, err
		}
		out = append(out, o)
	}
	return out, rows.Err()
}

// ReplaceObjects swaps the persisted loose-object snapshot (vehicles), not room data.
func (p *Postgres) ReplaceObjects(ctx context.Context, roomID uint64, objs []Object) error {
	tx, err := p.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx)
	if _, err := tx.Exec(ctx, `DELETE FROM room_objects WHERE room_id = $1`, roomID); err != nil {
		return err
	}
	for _, o := range objs {
		if _, err := tx.Exec(ctx,
			`INSERT INTO room_objects (room_id, object_id, kind, def, state, spawned_by) VALUES ($1, $2, $3, $4, $5, $6)`,
			roomID, o.ObjectID, o.Kind, o.Def, o.State, o.SpawnedBy); err != nil {
			return err
		}
	}
	return tx.Commit(ctx)
}

func (p *Postgres) LogEvent(ctx context.Context, e Event) error {
	detail := e.Detail
	if detail == "" {
		detail = "{}"
	}
	var target any
	if e.TargetID != 0 {
		target = e.TargetID
	}
	_, err := p.pool.Exec(ctx,
		`INSERT INTO room_events (room_id, actor_id, action, target_id, detail) VALUES ($1, $2, $3, $4, $5::jsonb)`,
		e.RoomID, e.ActorID, e.Action, target, detail)
	return err
}

func (p *Postgres) Events(ctx context.Context, roomID uint64, limit int) ([]Event, error) {
	rows, err := p.pool.Query(ctx, `
		SELECT created_at, room_id, actor_id, action, COALESCE(target_id, 0), detail::text
		FROM room_events WHERE room_id = $1 ORDER BY id DESC LIMIT $2`, roomID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Event
	for rows.Next() {
		var e Event
		if err := rows.Scan(&e.At, &e.RoomID, &e.ActorID, &e.Action, &e.TargetID, &e.Detail); err != nil {
			return nil, err
		}
		out = append(out, e)
	}
	return out, rows.Err()
}
