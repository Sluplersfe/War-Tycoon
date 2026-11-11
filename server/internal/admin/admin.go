// Package admin holds host-only command rules and room settings validation.
package admin

import (
	"errors"

	"google.golang.org/protobuf/proto"

	"frontline/internal/pb"
)

var (
	ErrNotHost     = errors.New("only the room host can do that")
	ErrSelf        = errors.New("you cannot target yourself")
	ErrBadSettings = errors.New("invalid settings")
	ErrBadCommand  = errors.New("invalid admin command")
)

const (
	MaxPlayersLimit = 16
	MaxCashGrant    = 1_000_000_000
	MaxRank         = 100
)

func DefaultSettings() *pb.RoomSettings {
	return &pb.RoomSettings{
		MaxPlayers:     8,
		FriendlyFire:   false,
		Teams:          []*pb.Team{{Name: "Rust", Color: 0xC8642D}, {Name: "Tide", Color: 0x2D9C9C}},
		CashMultiplier: 1,
		StartingCash:   100,
		KillReward:     150,
	}
}

// Normalize fills zero values from defaults and clamps out-of-range numbers.
// plots is the number of base plots on the map, which caps max_players.
func Normalize(s *pb.RoomSettings, plots int) (*pb.RoomSettings, error) {
	d := DefaultSettings()
	if s == nil {
		s = d
	}
	s = proto.Clone(s).(*pb.RoomSettings)
	if s.MaxPlayers == 0 {
		s.MaxPlayers = d.MaxPlayers
	}
	s.MaxPlayers = min(s.MaxPlayers, MaxPlayersLimit, uint32(plots))
	if len(s.Teams) == 0 {
		s.Teams = d.Teams
	}
	if len(s.Teams) > 4 {
		return nil, ErrBadSettings
	}
	for _, t := range s.Teams {
		if t.Name == "" || len(t.Name) > 24 || t.Color > 0xFFFFFF {
			return nil, ErrBadSettings
		}
	}
	if s.CashMultiplier <= 0 {
		s.CashMultiplier = 1
	}
	s.CashMultiplier = min(s.CashMultiplier, 100)
	if s.StartingCash < 0 || s.StartingCash > MaxCashGrant || s.KillReward < 0 || s.KillReward > MaxCashGrant {
		return nil, ErrBadSettings
	}
	return s, nil
}

// Authorize checks a command issued by actor in a room hosted by host.
func Authorize(actor, host uint64, cmd *pb.AdminCommand) error {
	if cmd == nil || cmd.Cmd == nil {
		return ErrBadCommand
	}
	if actor != host {
		return ErrNotHost
	}
	switch c := cmd.Cmd.(type) {
	case *pb.AdminCommand_Kick:
		return notSelf(actor, c.Kick)
	case *pb.AdminCommand_Ban:
		return notSelf(actor, c.Ban)
	case *pb.AdminCommand_TransferHost:
		return notSelf(actor, c.TransferHost)
	case *pb.AdminCommand_GiveCash:
		if c.GiveCash.Amount < -MaxCashGrant || c.GiveCash.Amount > MaxCashGrant {
			return ErrBadCommand
		}
	case *pb.AdminCommand_SetRank:
		if c.SetRank.Rank > MaxRank {
			return ErrBadCommand
		}
	}
	return nil
}

func notSelf(actor, target uint64) error {
	if actor == target {
		return ErrSelf
	}
	return nil
}
