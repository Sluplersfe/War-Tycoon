-- +goose Up
CREATE TABLE users (
    id             BIGSERIAL PRIMARY KEY,
    username       TEXT        NOT NULL,
    username_lower TEXT        NOT NULL UNIQUE,
    password_hash  TEXT        NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY,
    user_id    BIGINT      NOT NULL REFERENCES users (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX sessions_user_idx ON sessions (user_id);

-- Rooms are never deleted, only archived.
CREATE TABLE rooms (
    id             BIGSERIAL PRIMARY KEY,
    name           TEXT        NOT NULL,
    host_id        BIGINT      NOT NULL REFERENCES users (id),
    settings       JSONB       NOT NULL,
    next_object_id BIGINT      NOT NULL DEFAULT 1,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at    TIMESTAMPTZ
);

CREATE TABLE room_members (
    room_id   BIGINT      NOT NULL REFERENCES rooms (id),
    user_id   BIGINT      NOT NULL REFERENCES users (id),
    banned    BOOLEAN     NOT NULL DEFAULT false,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (room_id, user_id)
);

CREATE TABLE room_players (
    room_id    BIGINT      NOT NULL REFERENCES rooms (id),
    user_id    BIGINT      NOT NULL REFERENCES users (id),
    team       INT         NOT NULL DEFAULT 0,
    cash       BIGINT      NOT NULL DEFAULT 0,
    xp         BIGINT      NOT NULL DEFAULT 0,
    rank       INT         NOT NULL DEFAULT 0,
    rebirths   INT         NOT NULL DEFAULT 0,
    kills      BIGINT      NOT NULL DEFAULT 0,
    deaths     BIGINT      NOT NULL DEFAULT 0,
    base_state JSONB       NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (room_id, user_id)
);

-- Objects kept across sessions when the room setting persist_vehicles is on.
CREATE TABLE room_objects (
    room_id   BIGINT NOT NULL REFERENCES rooms (id),
    object_id BIGINT NOT NULL,
    kind      INT    NOT NULL,
    def       TEXT   NOT NULL,
    state     JSONB  NOT NULL,
    spawned_by BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (room_id, object_id)
);

CREATE TABLE room_events (
    id         BIGSERIAL PRIMARY KEY,
    room_id    BIGINT      NOT NULL REFERENCES rooms (id),
    actor_id   BIGINT      NOT NULL,
    action     TEXT        NOT NULL,
    target_id  BIGINT,
    detail     JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX room_events_room_idx ON room_events (room_id, created_at);

-- +goose Down
DROP TABLE room_events;
DROP TABLE room_objects;
DROP TABLE room_players;
DROP TABLE room_members;
DROP TABLE rooms;
DROP TABLE sessions;
DROP TABLE users;
