// Package ownership tracks which user owns each networked object in a room.
//
// Exactly one owner per object: a user id or Server (0). Only the owner may
// send state for an object; each ownership change bumps the epoch so packets
// from the previous owner are dropped. Server-owned objects are moved by the
// lightweight simulation in sim.go. Registry is not safe for concurrent use;
// the owning room serialises access.
package ownership

import (
	"errors"
	"time"

	"google.golang.org/protobuf/proto"

	"frontline/internal/pb"
	"frontline/internal/world"
)

const Server uint64 = 0

var (
	ErrNoObject   = errors.New("no such object")
	ErrSeatTaken  = errors.New("seat taken")
	ErrBadSeat    = errors.New("no such seat")
	ErrNotOwner   = errors.New("not the owner")
	ErrNotSeated  = errors.New("not seated")
	ErrNotClaimed = errors.New("object cannot be claimed")
)

type Object struct {
	ID        uint64
	Kind      pb.ObjectKind
	Def       string
	Owner     uint64
	Epoch     uint32
	State     *pb.ObjectState
	Seats     []uint64
	SpawnedBy uint64

	// Server sim bookkeeping.
	Asleep    bool
	stillFor  float32
	Abandoned time.Time // when it last became unoccupied and server-owned
	Dirty     bool      // state changed since last broadcast
}

func (o *Object) Info() *pb.ObjectInfo {
	return &pb.ObjectInfo{
		Id: o.ID, Kind: o.Kind, Def: o.Def, Owner: o.Owner, Epoch: o.Epoch,
		State: proto.Clone(o.State).(*pb.ObjectState), Seats: append([]uint64(nil), o.Seats...),
		SpawnedBy: o.SpawnedBy,
	}
}

// Change is an ownership or seating change that must be broadcast reliably.
type Change struct {
	Object *Object
}

func (c Change) Msg() *pb.OwnershipChanged {
	o := c.Object
	return &pb.OwnershipChanged{
		Id: o.ID, Owner: o.Owner, Epoch: o.Epoch,
		LastState: proto.Clone(o.State).(*pb.ObjectState), Seats: append([]uint64(nil), o.Seats...),
	}
}

type Registry struct {
	World   *world.World
	Objects map[uint64]*Object
	NextID  uint64
	now     func() time.Time
}

func NewRegistry(w *world.World, nextID uint64) *Registry {
	if nextID == 0 {
		nextID = 1
	}
	return &Registry{World: w, Objects: map[uint64]*Object{}, NextID: nextID, now: time.Now}
}

func (r *Registry) Get(id uint64) *Object { return r.Objects[id] }

// Spawn creates an object owned by owner (Server for unowned).
func (r *Registry) Spawn(kind pb.ObjectKind, def string, owner, spawnedBy uint64, state *pb.ObjectState) *Object {
	if state == nil {
		state = &pb.ObjectState{}
	}
	state = proto.Clone(state).(*pb.ObjectState)
	if state.Pos == nil {
		state.Pos = &pb.Vec3{}
	}
	if state.Rot == nil {
		state.Rot = &pb.Quat{W: 1}
	}
	seats := 0
	if kind == pb.ObjectKind_OBJECT_KIND_VEHICLE || kind == pb.ObjectKind_OBJECT_KIND_PROP {
		seats = r.World.Vehicle(def).Seats
		if state.Health == 0 {
			state.Health = r.World.Vehicle(def).Health
		}
	}
	o := &Object{
		ID: r.NextID, Kind: kind, Def: def, Owner: owner, Epoch: 1, State: state,
		Seats: make([]uint64, seats), SpawnedBy: spawnedBy, Dirty: true,
	}
	if owner == Server {
		o.Abandoned = r.now()
	}
	r.NextID++
	r.Objects[o.ID] = o
	return o
}

// Restore re-inserts a persisted object with a known id (server-owned).
func (r *Registry) Restore(id uint64, kind pb.ObjectKind, def string, spawnedBy uint64, state *pb.ObjectState) *Object {
	o := &Object{
		ID: id, Kind: kind, Def: def, Owner: Server, Epoch: 1, State: state,
		Seats: make([]uint64, r.World.Vehicle(def).Seats), SpawnedBy: spawnedBy, Abandoned: r.now(),
	}
	r.Objects[id] = o
	if id >= r.NextID {
		r.NextID = id + 1
	}
	return o
}

func (r *Registry) Despawn(id uint64) bool {
	if _, ok := r.Objects[id]; !ok {
		return false
	}
	delete(r.Objects, id)
	return true
}

// ApplyUpdate accepts state from sender only if it owns the object at the current epoch.
func (r *Registry) ApplyUpdate(sender uint64, u *pb.ObjectUpdate) bool {
	o := r.Objects[u.GetId()]
	if o == nil || sender == Server || o.Owner != sender || u.GetEpoch() != o.Epoch || u.GetState() == nil {
		return false
	}
	o.State = u.State
	o.Asleep = false
	return true
}

func (r *Registry) setOwner(o *Object, owner uint64) {
	if o.Owner == owner {
		return
	}
	o.Owner = owner
	o.Epoch++
	o.Asleep = false
	o.stillFor = 0
	if owner == Server {
		o.Abandoned = r.now()
	}
}

// Claim seats user in seat. Seat 0 is the driver and takes ownership.
// First request wins: an occupied seat is refused.
func (r *Registry) Claim(id, user uint64, seat uint32) ([]Change, error) {
	o := r.Objects[id]
	if o == nil {
		return nil, ErrNoObject
	}
	if o.Kind == pb.ObjectKind_OBJECT_KIND_PLAYER {
		return nil, ErrNotClaimed
	}
	if len(o.Seats) == 0 {
		// Loose props are picked up rather than seated.
		if o.Owner != Server && o.Owner != user {
			return nil, ErrSeatTaken
		}
		r.setOwner(o, user)
		return []Change{{o}}, nil
	}
	if int(seat) >= len(o.Seats) {
		return nil, ErrBadSeat
	}
	if o.Seats[seat] != 0 && o.Seats[seat] != user {
		return nil, ErrSeatTaken
	}
	if seat == 0 && o.Owner != Server && o.Owner != user {
		// Someone else controls it without the seat record (e.g. admin reassign).
		return nil, ErrSeatTaken
	}
	// A user sits in one seat at a time, across all vehicles.
	changes := r.unseatElsewhere(user, id)
	for i, s := range o.Seats {
		if s == user {
			o.Seats[i] = 0
		}
	}
	o.Seats[seat] = user
	if seat == 0 {
		r.setOwner(o, user)
	} else if o.Seats[0] == 0 && o.Owner == user {
		// Moved from driver to passenger: give the vehicle back to the server.
		r.setOwner(o, Server)
	}
	return append(changes, Change{o}), nil
}

func (r *Registry) unseatElsewhere(user, except uint64) []Change {
	var out []Change
	for _, o := range r.Objects {
		if o.ID == except {
			continue
		}
		touched := false
		for i, s := range o.Seats {
			if s == user {
				o.Seats[i] = 0
				touched = true
				if i == 0 {
					r.setOwner(o, Server)
				}
			}
		}
		if touched {
			out = append(out, Change{o})
		}
	}
	return out
}

// Release removes user from the object's seats; the driver leaving hands the
// object to Server, starting from final (if given).
func (r *Registry) Release(id, user uint64, final *pb.ObjectState) (Change, error) {
	o := r.Objects[id]
	if o == nil {
		return Change{}, ErrNoObject
	}
	seated := false
	for i, s := range o.Seats {
		if s == user {
			o.Seats[i] = 0
			seated = true
		}
	}
	if !seated && o.Owner != user {
		return Change{}, ErrNotSeated
	}
	if o.Owner == user && o.Kind != pb.ObjectKind_OBJECT_KIND_PLAYER {
		if final != nil {
			o.State = final
		}
		r.setOwner(o, Server)
	}
	return Change{o}, nil
}

// ReleaseAll is called when user disconnects: their vehicles and props go to
// Server, their seats are cleared and their character is removed.
func (r *Registry) ReleaseAll(user uint64) (changes []Change, despawned []uint64) {
	for _, o := range r.Objects {
		if o.Kind == pb.ObjectKind_OBJECT_KIND_PLAYER && o.Owner == user {
			delete(r.Objects, o.ID)
			despawned = append(despawned, o.ID)
			continue
		}
		touched := false
		for i, s := range o.Seats {
			if s == user {
				o.Seats[i] = 0
				touched = true
			}
		}
		if o.Owner == user {
			r.setOwner(o, Server)
			touched = true
		}
		if touched {
			changes = append(changes, Change{o})
		}
	}
	return changes, despawned
}

// Reassign is the host override. Seats are cleared when the owner changes so
// the old driver cannot keep sending updates.
func (r *Registry) Reassign(id, owner uint64) (Change, error) {
	o := r.Objects[id]
	if o == nil {
		return Change{}, ErrNoObject
	}
	if o.Kind == pb.ObjectKind_OBJECT_KIND_PLAYER {
		return Change{}, ErrNotClaimed
	}
	if o.Owner != owner {
		for i := range o.Seats {
			o.Seats[i] = 0
		}
	}
	r.setOwner(o, owner)
	return Change{o}, nil
}

// OwnedBy lists objects a user currently owns.
func (r *Registry) OwnedBy(user uint64) []*Object {
	var out []*Object
	for _, o := range r.Objects {
		if o.Owner == user {
			out = append(out, o)
		}
	}
	return out
}

func (r *Registry) Snapshot() []*pb.ObjectInfo {
	out := make([]*pb.ObjectInfo, 0, len(r.Objects))
	for _, o := range r.Objects {
		out = append(out, o.Info())
	}
	return out
}
