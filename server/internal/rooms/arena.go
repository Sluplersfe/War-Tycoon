package rooms

import (
	"fmt"
	"math"
	"math/rand"
	"time"

	"frontline/internal/ownership"
	"frontline/internal/pb"
)

// The arena gives every room something to fight over: server-driven raider
// bots that hunt players, and capture flags that pay the holding team.

const (
	botCount     = 6
	botRespawn   = 10 * time.Second
	botAggro     = 90.0
	botFireRange = 70.0
	botSpeed     = 4.2
	botWander    = 2.4
	botEyeHeight = 1.45
	flagRadius   = 14.0
	captureRate  = 9.0 // progress per second for one capturer
	incomeEvery  = 15 * time.Second
	flagIncome   = 35
)

// Capture flag layout: x, z and display name (the client holds the same names).
var flagSites = []struct {
	X, Z float32
	Name string
}{
	{0, -45, "Radio Tower"},
	{62, 62, "Town"},
	{-70, -70, "Airfield"},
	{60, -110, "Quarry"},
	{-70, 100, "Forest"},
}

type bot struct {
	id        uint64
	nextShot  time.Time
	waypoint  [2]float32
	wanderEnd time.Time
	strafe    float32
	dead      bool
	respawnAt time.Time
}

type arena struct {
	bots    map[uint64]*bot
	flags   []*ownership.Object
	nextPay time.Time
	pending []time.Time
	rng     *rand.Rand
}

func (r *Room) arenaInit() {
	if r.arena != nil {
		return
	}
	a := &arena{bots: map[uint64]*bot{}, rng: rand.New(rand.NewSource(int64(r.id)*7919 + 1)), nextPay: time.Now().Add(incomeEvery)}
	r.arena = a
	w := r.m.world
	for i, f := range flagSites {
		o := r.reg.Spawn(pb.ObjectKind_OBJECT_KIND_PROP, "flag", ownership.Server, 0, &pb.ObjectState{
			Pos:    &pb.Vec3{X: f.X, Y: w.GroundHeight(f.X, 200, f.Z) + 0.1, Z: f.Z},
			Health: 1000,
			// owner team (-1 none), progress 0..100, contesting team (-1 none), site index
			Extra: []float32{-1, 0, -1, float32(i)},
		})
		o.Static, o.Asleep = true, true
		a.flags = append(a.flags, o)
	}
	for i := 0; i < botCount; i++ {
		r.spawnBot()
	}
}

func (r *Room) randomOpenSpot() (float32, float32) {
	w := r.m.world
	for i := 0; i < 60; i++ {
		x := float32(r.arena.rng.Float64()*2-1) * (w.Map.HalfSize - 40)
		z := float32(r.arena.rng.Float64()*2-1) * (w.Map.HalfSize - 40)
		if w.Open(x, z, 4) && math.Hypot(float64(x), float64(z)) < 220 {
			return x, z
		}
	}
	return 40, 40
}

func (r *Room) spawnBot() {
	x, z := r.randomOpenSpot()
	o := r.reg.Spawn(pb.ObjectKind_OBJECT_KIND_PLAYER, "bot", ownership.Server, 0, &pb.ObjectState{
		Pos:    &pb.Vec3{X: x, Y: r.m.world.GroundHeight(x, 200, z) + 1.0, Z: z},
		Rot:    yawQuat(float32(r.arena.rng.Float64() * 6.28)),
		Health: 100,
		Extra:  []float32{0, 2, 0, 0},
	})
	o.Asleep = true
	r.arena.bots[o.ID] = &bot{id: o.ID, nextShot: time.Now().Add(time.Second), strafe: 1}
	r.broadcast(&pb.ServerMsg{Msg: &pb.ServerMsg_ObjectSpawned{ObjectSpawned: &pb.ObjectSpawned{Object: o.Info()}}}, 0)
}

// botKilled is called when damage reduces a bot to zero.
func (r *Room) botKilled(id uint64) {
	if b := r.arena.bots[id]; b != nil {
		b.dead = true
		b.respawnAt = time.Now().Add(botRespawn)
		delete(r.arena.bots, id)
		r.arena.pending = append(r.arena.pending, b.respawnAt)
	}
}

func (a *arena) isBot(id uint64) bool { _, ok := a.bots[id]; return ok }

// stepArena moves bots and updates flags; returns objects to broadcast.
func (r *Room) stepArena(dt float32) []*ownership.Object {
	if r.arena == nil || len(r.conns) == 0 {
		return nil
	}
	a := r.arena
	now := time.Now()
	var moved []*ownership.Object

	// Respawns.
	for len(a.pending) > 0 && now.After(a.pending[0]) {
		a.pending = a.pending[1:]
		r.spawnBot()
	}

	// Candidate targets: living, online players' avatars.
	type target struct {
		uid uint64
		o   *ownership.Object
	}
	var targets []target
	for uid, c := range r.conns {
		if o := r.reg.Get(c.avatar); o != nil && o.State.Health > 0 {
			targets = append(targets, target{uid, o})
		}
	}

	w := r.m.world
	for id, b := range a.bots {
		o := r.reg.Get(id)
		if o == nil {
			delete(a.bots, id)
			continue
		}
		p := o.State.Pos
		var best *target
		bd := float32(botAggro * botAggro)
		for i := range targets {
			t := &targets[i]
			if t.o.State.Extra != nil && len(t.o.State.Extra) > 2 && t.o.State.Extra[2] > 0.5 {
				// seated players are still valid targets, just harder to see; keep them
			}
			dx, dz := t.o.State.Pos.X-p.X, t.o.State.Pos.Z-p.Z
			if d := dx*dx + dz*dz; d < bd {
				bd, best = d, t
			}
		}
		var mx, mz float32
		speed := float32(botWander)
		yaw := float32(math.Atan2(float64(o.State.Rot.Y*2*o.State.Rot.W), float64(1-2*o.State.Rot.Y*o.State.Rot.Y)))
		if best != nil {
			tp := best.o.State.Pos
			dx, dz := tp.X-p.X, tp.Z-p.Z
			dist := float32(math.Sqrt(float64(bd)))
			yaw = float32(math.Atan2(float64(dx), float64(dz)))
			fx, fz := dx/max(dist, 0.01), dz/max(dist, 0.01)
			speed = botSpeed
			switch {
			case dist > 28:
				mx, mz = fx, fz
			case dist < 12:
				mx, mz = -fx, -fz
			default:
				mx, mz = -fz*b.strafe, fx*b.strafe
				speed = 2.4
			}
			if a.rng.Float32() < dt*0.4 {
				b.strafe = -b.strafe
			}
			// Shoot.
			eye := [3]float32{p.X, p.Y + botEyeHeight - 1.0, p.Z}
			tgt := [3]float32{tp.X, tp.Y + 0.5, tp.Z}
			if dist < botFireRange && now.After(b.nextShot) && !w.Blocked(eye, tgt) {
				b.nextShot = now.Add(time.Duration(450+a.rng.Intn(500)) * time.Millisecond)
				r.botShoot(o, best.uid, eye, tgt, dist)
			}
		} else {
			if now.After(b.wanderEnd) || (p.X-b.waypoint[0])*(p.X-b.waypoint[0])+(p.Z-b.waypoint[1])*(p.Z-b.waypoint[1]) < 9 {
				b.waypoint[0], b.waypoint[1] = r.randomOpenSpot()
				b.wanderEnd = now.Add(time.Duration(8+a.rng.Intn(8)) * time.Second)
			}
			dx, dz := b.waypoint[0]-p.X, b.waypoint[1]-p.Z
			d := float32(math.Hypot(float64(dx), float64(dz)))
			if d > 0.1 {
				mx, mz = dx/d, dz/d
				yaw = float32(math.Atan2(float64(dx), float64(dz)))
			}
		}
		// Move with simple box avoidance (axis-separated).
		nx, nz := p.X+mx*speed*dt, p.Z+mz*speed*dt
		if w.Open(nx, p.Z, 0.7) {
			p.X = nx
		} else {
			b.wanderEnd = now // pick a new waypoint
			b.strafe = -b.strafe
		}
		if w.Open(p.X, nz, 0.7) {
			p.Z = nz
		} else {
			b.wanderEnd = now
			b.strafe = -b.strafe
		}
		p.Y = w.GroundHeight(p.X, p.Y+1, p.Z) + 1.0
		o.State.Vel = &pb.Vec3{X: mx * speed, Z: mz * speed}
		o.State.Rot = yawQuat(yaw)
		o.State.Extra[0] = 0
		o.Dirty = true
		moved = append(moved, o)
	}

	r.stepFlags(dt, now, &moved)
	return moved
}

// botShoot rolls a hit and tells the target's client to apply the damage.
func (r *Room) botShoot(bot *ownership.Object, uid uint64, eye, tgt [3]float32, dist float32) {
	chance := 0.62 - dist/150
	if chance < 0.12 {
		chance = 0.12
	}
	hit := r.arena.rng.Float32() <= chance
	dir := &pb.Vec3{X: tgt[0] - eye[0], Y: tgt[1] - eye[1], Z: tgt[2] - eye[2]}
	if !hit {
		// A miss is drawn as a shot that goes wide, so players can see and hear it.
		off := func() float32 { return (r.arena.rng.Float32() - 0.5) * (1.2 + dist*0.06) }
		dir.X += off()
		dir.Y += off() * 0.5
		dir.Z += off()
	}
	r.sendUDPAll(&pb.UdpPacket{Tick: r.tick, Shots: []*pb.ProjectileFired{{
		Shooter: 0, Weapon: "rifle", Origin: &pb.Vec3{X: eye[0], Y: eye[1], Z: eye[2]}, Dir: dir,
	}}}, 0)
	if !hit {
		return
	}
	c := r.conns[uid]
	if c == nil {
		return
	}
	c.peer.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_Hit{Hit: &pb.HitReceived{
		From: 0, TargetId: c.avatar, Damage: 6 + r.arena.rng.Float32()*5, Weapon: "bot_rifle",
		Point: &pb.Vec3{X: bot.State.Pos.X, Y: bot.State.Pos.Y, Z: bot.State.Pos.Z},
	}}})
}

func (r *Room) stepFlags(dt float32, now time.Time, moved *[]*ownership.Object) {
	a := r.arena
	pay := now.After(a.nextPay)
	if pay {
		a.nextPay = now.Add(incomeEvery)
	}
	income := map[uint64]int64{}
	for _, f := range a.flags {
		counts := map[uint32]int{}
		var present []uint64
		for uid, c := range r.conns {
			av := r.reg.Get(c.avatar)
			pl := r.players[uid]
			if av == nil || pl == nil || av.State.Health <= 0 {
				continue
			}
			dx, dz := av.State.Pos.X-f.State.Pos.X, av.State.Pos.Z-f.State.Pos.Z
			if dx*dx+dz*dz <= flagRadius*flagRadius {
				counts[pl.Team]++
				present = append(present, uid)
			}
		}
		ex := f.State.Extra
		owner, prog := int(ex[0]), ex[1]
		changed := false
		if len(counts) == 1 {
			var team uint32
			var n int
			for t, k := range counts {
				team, n = t, k
			}
			step := captureRate * dt * float32(math.Sqrt(float64(n)))
			switch {
			case owner == int(team):
				if prog < 100 {
					prog = min(100, prog+step)
					changed = true
				}
				ex[2] = -1
			default:
				ex[2] = float32(team)
				prog -= step
				changed = true
				if prog <= 0 {
					owner, prog = int(team), 0
					ex[2] = -1
					r.flagCaptured(f, team, present)
				}
			}
		} else if len(counts) > 1 {
			ex[2] = -2 // contested
			changed = true
		} else if ex[2] != -1 {
			ex[2] = -1
			changed = true
		}
		ex[0], ex[1] = float32(owner), prog
		if changed {
			f.Dirty = true
			*moved = append(*moved, f)
		}
		if pay && owner >= 0 && prog >= 50 {
			for uid := range r.conns {
				if pl := r.players[uid]; pl != nil && int(pl.Team) == owner {
					income[uid] += int64(float32(flagIncome) * r.settings.CashMultiplier)
				}
			}
		}
	}
	for uid, cash := range income {
		if c := r.conns[uid]; c != nil {
			c.peer.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_Reward{Reward: &pb.Reward{Cash: cash, Xp: 4, Reason: "flag income"}}})
		}
	}
}

func (r *Room) flagCaptured(f *ownership.Object, team uint32, present []uint64) {
	name := flagSites[int(f.State.Extra[3])%len(flagSites)].Name
	teamName := fmt.Sprintf("team %d", team)
	if int(team) < len(r.settings.Teams) {
		teamName = r.settings.Teams[team].Name
	}
	r.systemChat(fmt.Sprintf("%s captured %s", teamName, name))
	for _, uid := range present {
		if pl := r.players[uid]; pl != nil && pl.Team == team {
			if c := r.conns[uid]; c != nil {
				c.peer.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_Reward{Reward: &pb.Reward{
					Cash: int64(float32(150) * r.settings.CashMultiplier), Xp: 60, Reason: "captured " + name,
				}}})
			}
		}
	}
}
