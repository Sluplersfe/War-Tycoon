package ownership

import (
	"math"
	"time"

	"frontline/internal/pb"
)

const (
	gravity        = 9.81
	groundFriction = 3.0  // parking brake: horizontal velocity decay per second on ground
	airDrag        = 0.05 // per second
	angDamping     = 2.0
	uprightRate    = 2.0
	sleepSpeed     = 0.05
	sleepAfter     = 0.5 // seconds still before sleeping
	worldFloor     = -50
)

// Step advances every awake, server-owned, non-player object by dt seconds.
// It returns the objects whose state should be broadcast and the ids of objects
// removed because they were abandoned for longer than abandonTTL (0 = never).
func (r *Registry) Step(dt float32, abandonTTL time.Duration) (moved []*Object, despawned []uint64) {
	now := r.now()
	for id, o := range r.Objects {
		if o.Owner != Server || o.Kind == pb.ObjectKind_OBJECT_KIND_PLAYER {
			continue
		}
		if abandonTTL > 0 && !o.occupied() && now.Sub(o.Abandoned) > abandonTTL {
			delete(r.Objects, id)
			despawned = append(despawned, id)
			continue
		}
		if o.Asleep {
			continue
		}
		r.integrate(o, dt)
		moved = append(moved, o)
	}
	return moved, despawned
}

func (o *Object) occupied() bool {
	for _, s := range o.Seats {
		if s != 0 {
			return true
		}
	}
	return false
}

func (r *Registry) integrate(o *Object, dt float32) {
	s := o.State
	if s.Vel == nil {
		s.Vel = &pb.Vec3{}
	}
	if s.Angvel == nil {
		s.Angvel = &pb.Vec3{}
	}
	he := r.World.Vehicle(o.Def).HalfExtents
	p, v := s.Pos, s.Vel

	v.Y -= gravity * dt
	p.X += v.X * dt
	p.Y += v.Y * dt
	p.Z += v.Z * dt

	// Ground (floor plane or top of a static box).
	ground := r.World.GroundHeight(p.X, p.Y-he[1], p.Z)
	grounded := false
	if p.Y-he[1] <= ground {
		p.Y = ground + he[1]
		if v.Y < 0 {
			v.Y = 0
		}
		grounded = true
	}
	if p.Y < worldFloor {
		p.Y = worldFloor
		v.Y = 0
	}

	// Push out of static boxes sideways.
	rad := max(he[0], he[2])
	for _, b := range r.World.Map.Statics {
		if p.Y-he[1] >= b.Max[1]-0.3 || p.Y+he[1] <= b.Min[1] {
			continue
		}
		dxMin := (p.X + rad) - b.Min[0]
		dxMax := b.Max[0] - (p.X - rad)
		dzMin := (p.Z + rad) - b.Min[2]
		dzMax := b.Max[2] - (p.Z - rad)
		if dxMin <= 0 || dxMax <= 0 || dzMin <= 0 || dzMax <= 0 {
			continue
		}
		switch m := min(dxMin, dxMax, dzMin, dzMax); m {
		case dxMin:
			p.X -= dxMin
			v.X = min(v.X, 0)
		case dxMax:
			p.X += dxMax
			v.X = max(v.X, 0)
		case dzMin:
			p.Z -= dzMin
			v.Z = min(v.Z, 0)
		default:
			p.Z += dzMax
			v.Z = max(v.Z, 0)
		}
	}

	decay := float32(airDrag)
	if grounded {
		decay = groundFriction
	}
	k := max(0, 1-decay*dt)
	v.X *= k
	v.Z *= k

	w := s.Angvel
	ak := max(0, 1-angDamping*dt)
	w.X *= ak
	w.Y *= ak
	w.Z *= ak
	s.Rot = integrateRot(s.Rot, w, dt)
	if grounded {
		s.Rot = slerp(s.Rot, yawOnly(s.Rot), min(1, uprightRate*dt))
	}

	speed := float32(math.Sqrt(float64(v.X*v.X + v.Y*v.Y + v.Z*v.Z)))
	ang := float32(math.Sqrt(float64(w.X*w.X + w.Y*w.Y + w.Z*w.Z)))
	if grounded && speed < sleepSpeed && ang < sleepSpeed {
		o.stillFor += dt
		if o.stillFor >= sleepAfter {
			v.X, v.Y, v.Z = 0, 0, 0
			w.X, w.Y, w.Z = 0, 0, 0
			s.Rot = yawOnly(s.Rot)
			o.Asleep = true
		}
	} else {
		o.stillFor = 0
	}
	o.Dirty = true
}

func normalize(q *pb.Quat) *pb.Quat {
	n := float32(math.Sqrt(float64(q.X*q.X + q.Y*q.Y + q.Z*q.Z + q.W*q.W)))
	if n < 1e-6 {
		return &pb.Quat{W: 1}
	}
	return &pb.Quat{X: q.X / n, Y: q.Y / n, Z: q.Z / n, W: q.W / n}
}

// integrateRot applies world-space angular velocity w: q' = q + 0.5 * (w,0) * q * dt.
func integrateRot(q *pb.Quat, w *pb.Vec3, dt float32) *pb.Quat {
	if q == nil {
		q = &pb.Quat{W: 1}
	}
	h := 0.5 * dt
	return normalize(&pb.Quat{
		X: q.X + h*(w.X*q.W+w.Y*q.Z-w.Z*q.Y),
		Y: q.Y + h*(w.Y*q.W+w.Z*q.X-w.X*q.Z),
		Z: q.Z + h*(w.Z*q.W+w.X*q.Y-w.Y*q.X),
		W: q.W + h*(-w.X*q.X-w.Y*q.Y-w.Z*q.Z),
	})
}

// yawOnly keeps the heading of q (its local +Z projected onto the ground).
func yawOnly(q *pb.Quat) *pb.Quat {
	// forward = q * (0,0,1)
	fx := 2 * (q.X*q.Z + q.W*q.Y)
	fz := 1 - 2*(q.X*q.X+q.Y*q.Y)
	yaw := math.Atan2(float64(fx), float64(fz))
	s, c := math.Sincos(yaw / 2)
	return &pb.Quat{Y: float32(s), W: float32(c)}
}

func slerp(a, b *pb.Quat, t float32) *pb.Quat {
	dot := a.X*b.X + a.Y*b.Y + a.Z*b.Z + a.W*b.W
	bx, by, bz, bw := b.X, b.Y, b.Z, b.W
	if dot < 0 {
		dot, bx, by, bz, bw = -dot, -bx, -by, -bz, -bw
	}
	// Normalised lerp is accurate enough for small steps.
	return normalize(&pb.Quat{
		X: a.X + (bx-a.X)*t, Y: a.Y + (by-a.Y)*t, Z: a.Z + (bz-a.Z)*t, W: a.W + (bw-a.W)*t,
	})
}
