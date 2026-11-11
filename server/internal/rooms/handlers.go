package rooms

import (
	"context"
	"encoding/json"
	"fmt"
	"math"
	"strings"
	"time"
	"unicode/utf8"

	"google.golang.org/protobuf/encoding/protojson"

	"frontline/internal/admin"
	"frontline/internal/ownership"
	"frontline/internal/pb"
)

func sincos(a float32) (float32, float32) {
	s, c := math.Sincos(float64(a))
	return float32(s), float32(c)
}

// Handle processes an in-room TCP message from p.
func (r *Room) Handle(p Peer, msg *pb.ClientMsg) {
	if a, ok := msg.Msg.(*pb.ClientMsg_Admin); ok {
		if arch, ok := a.Admin.Cmd.(*pb.AdminCommand_Archive); ok && arch.Archive {
			r.archive(p)
			return
		}
	}
	if _, ok := msg.Msg.(*pb.ClientMsg_ListEvents); ok {
		r.listEvents(p)
		return
	}

	r.mu.Lock()
	defer r.mu.Unlock()
	c := r.conns[p.UserID()]
	if c == nil || c.peer != p || r.closed {
		return
	}
	switch m := msg.Msg.(type) {
	case *pb.ClientMsg_Chat:
		r.chat(c, m.Chat)
	case *pb.ClientMsg_ChooseTeam:
		r.chooseTeam(c, m.ChooseTeam.Team)
	case *pb.ClientMsg_Spawn:
		r.spawn(c, m.Spawn)
	case *pb.ClientMsg_Despawn:
		r.despawn(c, m.Despawn)
	case *pb.ClientMsg_Claim:
		changes, err := r.reg.Claim(m.Claim.Id, p.UserID(), m.Claim.Seat)
		if err != nil {
			p.Send(errMsg("claim", err.Error()))
			return
		}
		r.broadcastChanges(changes)
	case *pb.ClientMsg_Release:
		ch, err := r.reg.Release(m.Release.Id, p.UserID(), m.Release.State)
		if err != nil {
			p.Send(errMsg("release", err.Error()))
			return
		}
		r.broadcastChanges([]ownership.Change{ch})
	case *pb.ClientMsg_SaveProgress:
		r.saveProgress(c, m.SaveProgress.Progress)
	case *pb.ClientMsg_Hit:
		r.hit(c, m.Hit)
	case *pb.ClientMsg_Death:
		r.death(c, m.Death)
	case *pb.ClientMsg_Admin:
		r.admin(c, m.Admin)
	default:
		p.Send(errMsg("bad_request", "unexpected message in room"))
	}
}

func (r *Room) chat(c *conn, m *pb.SendChat) {
	text := strings.TrimSpace(m.Text)
	if text == "" {
		return
	}
	if utf8.RuneCountInString(text) > 200 {
		text = string([]rune(text)[:200])
	}
	if !c.allow("chat", 5, 5*time.Second) {
		c.peer.Send(errMsg("rate", "slow down"))
		return
	}
	pl := r.players[c.peer.UserID()]
	r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_Chat{Chat: &pb.ChatMsg{
		From: pl.UserID, FromName: pl.Username, Text: text,
	}}}, 0)
}

func (r *Room) chooseTeam(c *conn, team uint32) {
	if int(team) >= len(r.settings.Teams) {
		c.peer.Send(errMsg("team", "no such team"))
		return
	}
	pl := r.players[c.peer.UserID()]
	if pl.Team == team {
		return
	}
	pl.Team = team
	pl.dirty = true
	r.playerUpdated(pl)
}

func (r *Room) spawn(c *conn, m *pb.SpawnObject) {
	uid := c.peer.UserID()
	if m.Kind != pb.ObjectKind_OBJECT_KIND_VEHICLE && m.Kind != pb.ObjectKind_OBJECT_KIND_PROP {
		c.peer.Send(errMsg("spawn", "cannot spawn that kind"))
		return
	}
	if _, ok := r.m.world.Vehicles[m.Def]; !ok {
		c.peer.Send(errMsg("spawn", "unknown object"))
		return
	}
	if !c.allow("spawn", 3, 3*time.Second) {
		c.peer.Send(errMsg("rate", "slow down"))
		return
	}
	n := 0
	for _, o := range r.reg.Objects {
		if o.SpawnedBy == uid && o.Kind != pb.ObjectKind_OBJECT_KIND_PLAYER {
			n++
		}
	}
	if n >= maxVehiclesPerUser {
		c.peer.Send(errMsg("spawn", fmt.Sprintf("you already have %d vehicles out", n)))
		return
	}
	owner := uid
	if m.Unowned {
		owner = ownership.Server
	}
	o := r.reg.Spawn(m.Kind, m.Def, owner, uid, m.State)
	if owner == uid && len(o.Seats) > 0 {
		o.Seats[0] = uid // spawning owned means spawning in the driver's seat
	}
	r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_ObjectSpawned{ObjectSpawned: &pb.ObjectSpawned{Object: o.Info()}}}, 0)
}

func (r *Room) despawn(c *conn, m *pb.DespawnObject) {
	uid := c.peer.UserID()
	o := r.reg.Get(m.Id)
	if o == nil || o.Kind == pb.ObjectKind_OBJECT_KIND_PLAYER {
		return
	}
	// The owner may remove its object; anyone may clear their own unoccupied spawn.
	if o.Owner != uid && !(o.Owner == ownership.Server && o.SpawnedBy == uid && !occupied(o)) {
		c.peer.Send(errMsg("despawn", "not yours"))
		return
	}
	r.reg.Despawn(o.ID)
	r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_ObjectDespawned{ObjectDespawned: &pb.ObjectDespawned{
		Id: o.ID, Destroyed: m.Destroyed,
	}}}, 0)
}

func occupied(o *ownership.Object) bool {
	for _, s := range o.Seats {
		if s != 0 {
			return true
		}
	}
	return false
}

func (r *Room) saveProgress(c *conn, p *pb.Progress) {
	if p == nil {
		return
	}
	if !c.allow("save", 10, 5*time.Second) {
		return
	}
	pl := r.players[c.peer.UserID()]
	base := p.BaseJson
	if base == "" {
		base = "{}"
	}
	if len(base) > maxBaseJSON || !json.Valid([]byte(base)) {
		c.peer.Send(errMsg("save", "base data rejected"))
		return
	}
	if p.Cash < 0 || p.Xp < 0 || p.Rank > admin.MaxRank || p.Rebirths > 10000 {
		c.peer.Send(errMsg("save", "progress out of range"))
		return
	}
	visible := pl.BaseState != base || pl.Rank != p.Rank || pl.Rebirths != p.Rebirths
	pl.Cash, pl.XP, pl.Rank, pl.Rebirths, pl.BaseState = p.Cash, p.Xp, p.Rank, p.Rebirths, base
	pl.dirty = true
	// Kills and deaths are counted by the server and ignored here.
	if visible {
		r.playerUpdated(pl)
	}
}

func (r *Room) teamOf(o *ownership.Object) (uint32, bool) {
	uid := o.Owner
	if uid == ownership.Server {
		uid = o.SpawnedBy
	}
	pl := r.players[uid]
	if pl == nil {
		return 0, false
	}
	return pl.Team, true
}

func (r *Room) hit(c *conn, h *pb.HitReport) {
	uid := c.peer.UserID()
	if !c.allow("hit", 40, time.Second) {
		return
	}
	o := r.reg.Get(h.TargetId)
	if o == nil || o.Owner == uid || h.Damage <= 0 || math.IsNaN(float64(h.Damage)) {
		return
	}
	dmg := min(h.Damage, 1000)
	if !r.settings.FriendlyFire {
		if t, ok := r.teamOf(o); ok && t == r.players[uid].Team {
			return
		}
	}
	if o.Owner != ownership.Server {
		if tc := r.conns[o.Owner]; tc != nil {
			tc.peer.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_Hit{Hit: &pb.HitReceived{
				From: uid, TargetId: o.ID, Damage: dmg, Weapon: h.Weapon, Point: h.Point,
			}}})
		}
		return
	}
	// Unowned: the server applies damage itself.
	o.State.Health -= dmg
	o.Asleep = false
	if o.State.Health <= 0 {
		r.reg.Despawn(o.ID)
		r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_ObjectDespawned{ObjectDespawned: &pb.ObjectDespawned{
			Id: o.ID, Destroyed: true,
		}}}, 0)
		reward := int64(float32(r.settings.KillReward) * r.settings.CashMultiplier / 2)
		c.peer.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_Reward{Reward: &pb.Reward{
			Cash: reward, Xp: 20, Reason: "Destroyed " + o.Def,
		}}})
	}
}

func (r *Room) death(c *conn, d *pb.DeathReport) {
	uid := c.peer.UserID()
	if !c.allow("death", 1, 2*time.Second) {
		return
	}
	victim := r.players[uid]
	victim.Deaths++
	victim.dirty = true
	r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_Kill{Kill: &pb.KillFeed{
		Killer: d.KillerId, Victim: uid, Weapon: d.Weapon,
	}}}, 0)
	r.playerUpdated(victim)
	killer := r.players[d.KillerId]
	if killer == nil || d.KillerId == uid {
		return
	}
	killer.Kills++
	killer.dirty = true
	r.playerUpdated(killer)
	if killer.Team == victim.Team {
		return // no reward for team kills
	}
	if kc := r.conns[killer.UserID]; kc != nil {
		kc.peer.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_Reward{Reward: &pb.Reward{
			Cash:   int64(float32(r.settings.KillReward) * r.settings.CashMultiplier),
			Xp:     50,
			Reason: "Eliminated " + victim.Username,
		}}})
	}
}

// ---------- host admin ----------

func (r *Room) admin(c *conn, cmd *pb.AdminCommand) {
	uid := c.peer.UserID()
	if err := admin.Authorize(uid, r.hostID, cmd); err != nil {
		c.peer.Send(errMsg("admin", err.Error()))
		return
	}
	fail := func(text string) { c.peer.Send(errMsg("admin", text)) }
	target := func(id uint64) *Player {
		pl := r.players[id]
		if pl == nil {
			fail("no such player in this room")
		}
		return pl
	}

	switch x := cmd.Cmd.(type) {
	case *pb.AdminCommand_Kick:
		tc := r.conns[x.Kick]
		if tc == nil {
			fail("player is not online")
			return
		}
		r.removeConn(tc, "kicked by the host")
		r.logEvent(uid, "kick", x.Kick, nil)

	case *pb.AdminCommand_Ban:
		pl := target(x.Ban)
		if pl == nil {
			return
		}
		if err := r.m.store.SetBanned(context.Background(), r.id, x.Ban, true); err != nil {
			fail("ban failed")
			return
		}
		pl.Banned = true
		if tc := r.conns[x.Ban]; tc != nil {
			r.removeConn(tc, "banned by the host")
		}
		r.playerUpdated(pl)
		r.logEvent(uid, "ban", x.Ban, nil)

	case *pb.AdminCommand_Unban:
		pl := target(x.Unban)
		if pl == nil {
			return
		}
		if err := r.m.store.SetBanned(context.Background(), r.id, x.Unban, false); err != nil {
			fail("unban failed")
			return
		}
		pl.Banned = false
		r.playerUpdated(pl)
		r.logEvent(uid, "unban", x.Unban, nil)

	case *pb.AdminCommand_GiveCash:
		pl := target(x.GiveCash.UserId)
		if pl == nil {
			return
		}
		if tc := r.conns[pl.UserID]; tc != nil {
			// The player's client owns its economy and applies the grant.
			tc.peer.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_Reward{Reward: &pb.Reward{
				Cash: x.GiveCash.Amount, Reason: "Host grant",
			}}})
		} else {
			pl.Cash = max(0, pl.Cash+x.GiveCash.Amount)
			pl.dirty = true
		}
		r.logEvent(uid, "give_cash", pl.UserID, map[string]int64{"amount": x.GiveCash.Amount})

	case *pb.AdminCommand_SetRank:
		pl := target(x.SetRank.UserId)
		if pl == nil {
			return
		}
		pl.Rank = x.SetRank.Rank
		pl.dirty = true
		r.overrideProgress(pl, "Host set your rank")
		r.logEvent(uid, "set_rank", pl.UserID, map[string]uint32{"rank": x.SetRank.Rank})

	case *pb.AdminCommand_ResetPlayer:
		pl := target(x.ResetPlayer)
		if pl == nil {
			return
		}
		pl.Cash, pl.XP, pl.Rank, pl.Rebirths, pl.BaseState = r.settings.StartingCash, 0, 0, 0, "{}"
		pl.Kills, pl.Deaths = 0, 0
		pl.dirty = true
		r.overrideProgress(pl, "Host reset your progress")
		r.logEvent(uid, "reset_player", pl.UserID, nil)

	case *pb.AdminCommand_UpdateSettings:
		s, err := admin.Normalize(x.UpdateSettings, len(r.plots))
		if err != nil {
			fail(err.Error())
			return
		}
		b, _ := protojson.Marshal(s)
		if err := r.m.store.SetRoomSettings(context.Background(), r.id, b); err != nil {
			fail("could not save settings")
			return
		}
		r.settings = s
		for _, pl := range r.players {
			if int(pl.Team) >= len(s.Teams) {
				pl.Team = 0
				pl.dirty = true
				r.playerUpdated(pl)
			}
		}
		r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_Settings{Settings: &pb.SettingsUpdated{Settings: s}}}, 0)
		r.logEvent(uid, "update_settings", 0, json.RawMessage(b))

	case *pb.AdminCommand_TransferHost:
		pl := target(x.TransferHost)
		if pl == nil {
			return
		}
		if err := r.m.store.SetRoomHost(context.Background(), r.id, pl.UserID); err != nil {
			fail("transfer failed")
			return
		}
		old := r.players[r.hostID]
		r.hostID = pl.UserID
		r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_HostChanged{HostChanged: &pb.HostChanged{HostId: pl.UserID}}}, 0)
		if old != nil {
			r.playerUpdated(old)
		}
		r.playerUpdated(pl)
		r.systemChat(pl.Username + " is now the host")
		r.logEvent(uid, "transfer_host", pl.UserID, nil)

	case *pb.AdminCommand_Archive:
		// archive=true is handled before locking; archive=false is a no-op on a live room.

	case *pb.AdminCommand_UnarchiveRoom:
		fail("unarchive rooms from the room list")

	case *pb.AdminCommand_Reassign:
		newOwner := x.Reassign.NewOwner
		if newOwner != ownership.Server && r.conns[newOwner] == nil {
			fail("new owner must be online")
			return
		}
		ch, err := r.reg.Reassign(x.Reassign.ObjectId, newOwner)
		if err != nil {
			fail(err.Error())
			return
		}
		r.broadcastChanges([]ownership.Change{ch})
		r.logEvent(uid, "reassign", newOwner, map[string]uint64{"object": x.Reassign.ObjectId})

	case *pb.AdminCommand_Despawn:
		o := r.reg.Get(x.Despawn)
		if o == nil || o.Kind == pb.ObjectKind_OBJECT_KIND_PLAYER {
			fail("no such object")
			return
		}
		r.reg.Despawn(o.ID)
		r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_ObjectDespawned{ObjectDespawned: &pb.ObjectDespawned{Id: o.ID}}}, 0)
		r.logEvent(uid, "despawn", 0, map[string]any{"object": o.ID, "def": o.Def})

	case *pb.AdminCommand_SetTeam:
		pl := target(x.SetTeam.UserId)
		if pl == nil {
			return
		}
		if int(x.SetTeam.Team) >= len(r.settings.Teams) {
			fail("no such team")
			return
		}
		pl.Team = x.SetTeam.Team
		pl.dirty = true
		r.playerUpdated(pl)
		r.logEvent(uid, "set_team", pl.UserID, map[string]uint32{"team": x.SetTeam.Team})
	}
}

func (r *Room) overrideProgress(pl *Player, reason string) {
	if tc := r.conns[pl.UserID]; tc != nil {
		tc.peer.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_ProgressOverride{ProgressOverride: &pb.ProgressOverride{
			Progress: r.progress(pl), Reason: reason,
		}}})
	}
	r.playerUpdated(pl)
}

func (r *Room) archive(p Peer) {
	r.mu.Lock()
	ok := !r.closed && p.UserID() == r.hostID && r.conns[p.UserID()] != nil
	r.mu.Unlock()
	if !ok {
		p.Send(errMsg("admin", admin.ErrNotHost.Error()))
		return
	}
	r.logEvent(p.UserID(), "archive", 0, nil)
	r.close(context.Background(), "the host archived this room", true)
}

func (r *Room) listEvents(p Peer) {
	r.mu.Lock()
	isHost := p.UserID() == r.hostID
	names := map[uint64]string{}
	for id, pl := range r.players {
		names[id] = pl.Username
	}
	r.mu.Unlock()
	if !isHost {
		p.Send(errMsg("admin", admin.ErrNotHost.Error()))
		return
	}
	evs, err := r.m.store.Events(context.Background(), r.id, 100)
	if err != nil {
		p.Send(errMsg("admin", "could not load log"))
		return
	}
	out := &pb.EventLog{}
	for _, e := range evs {
		out.Events = append(out.Events, &pb.EventEntry{
			TimeMs: e.At.UnixMilli(), ActorId: e.ActorID, ActorName: names[e.ActorID],
			Action: e.Action, TargetId: e.TargetID, Detail: e.Detail,
		})
	}
	p.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_EventLog{EventLog: out}})
}

// ---------- UDP ----------

// HandleUDP relays owner-sent state. Updates for objects the sender does not
// own, or with a stale epoch, are dropped here.
func (r *Room) HandleUDP(p Peer, pkt *pb.UdpPacket) {
	r.mu.Lock()
	defer r.mu.Unlock()
	uid := p.UserID()
	c := r.conns[uid]
	if c == nil || c.peer != p || r.closed {
		return
	}
	if !c.allow("udp", 90, time.Second) {
		return
	}
	out := &pb.UdpPacket{Tick: pkt.Tick}
	ups := pkt.Updates
	if len(ups) > maxUpdatesPerPacket {
		ups = ups[:maxUpdatesPerPacket]
	}
	for _, u := range ups {
		if r.reg.ApplyUpdate(uid, u) {
			out.Updates = append(out.Updates, u)
		}
	}
	shots := pkt.Shots
	if len(shots) > maxShotsPerPacket {
		shots = shots[:maxShotsPerPacket]
	}
	for _, s := range shots {
		s.Shooter = uid
		out.Shots = append(out.Shots, s)
	}
	if len(out.Updates) == 0 && len(out.Shots) == 0 {
		return
	}
	r.sendUDPAll(out, uid)
}
