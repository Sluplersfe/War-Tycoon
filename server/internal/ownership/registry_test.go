package ownership

import (
	"math"
	"testing"
	"time"

	"frontline/internal/pb"
	"frontline/internal/world"
)

const alice, bob uint64 = 10, 20

func testWorld() *world.World {
	w := &world.World{Vehicles: map[string]world.VehicleDef{
		"jeep": {Class: "car", Seats: 2, HalfExtents: [3]float32{1, 0.8, 2}, Health: 400},
	}}
	w.Map.Plots.Count = 8
	w.Map.Statics = []world.AABB{{Name: "wall", Min: [3]float32{10, 0, -5}, Max: [3]float32{12, 3, 5}}}
	return w
}

func update(o *Object, epoch uint32, x float32) *pb.ObjectUpdate {
	return &pb.ObjectUpdate{Id: o.ID, Epoch: epoch, State: &pb.ObjectState{Pos: &pb.Vec3{X: x}, Rot: &pb.Quat{W: 1}}}
}

func TestNonOwnerUpdateDropped(t *testing.T) {
	r := NewRegistry(testWorld(), 1)
	me := r.Spawn(pb.ObjectKind_OBJECT_KIND_PLAYER, "soldier", alice, alice, nil)
	if r.ApplyUpdate(bob, update(me, me.Epoch, 5)) {
		t.Fatal("bob must not update alice's character")
	}
	if !r.ApplyUpdate(alice, update(me, me.Epoch, 5)) || me.State.Pos.X != 5 {
		t.Fatal("owner update must apply")
	}
	jeep := r.Spawn(pb.ObjectKind_OBJECT_KIND_VEHICLE, "jeep", Server, alice, nil)
	if r.ApplyUpdate(Server, update(jeep, jeep.Epoch, 1)) || r.ApplyUpdate(alice, update(jeep, jeep.Epoch, 1)) {
		t.Fatal("nobody may send updates for a server-owned object")
	}
}

func TestStaleEpochDropped(t *testing.T) {
	r := NewRegistry(testWorld(), 1)
	jeep := r.Spawn(pb.ObjectKind_OBJECT_KIND_VEHICLE, "jeep", Server, alice, nil)
	if _, err := r.Claim(jeep.ID, alice, 0); err != nil {
		t.Fatal(err)
	}
	aliceEpoch := jeep.Epoch
	if _, err := r.Release(jeep.ID, alice, nil); err != nil {
		t.Fatal(err)
	}
	if _, err := r.Claim(jeep.ID, alice, 0); err != nil {
		t.Fatal(err)
	}
	if jeep.Epoch <= aliceEpoch {
		t.Fatal("epoch must increase on every ownership change")
	}
	if r.ApplyUpdate(alice, update(jeep, aliceEpoch, 9)) {
		t.Fatal("packet from a previous ownership epoch must be dropped")
	}
	if !r.ApplyUpdate(alice, update(jeep, jeep.Epoch, 9)) {
		t.Fatal("current epoch must apply")
	}
}

func TestSeatTransfer(t *testing.T) {
	r := NewRegistry(testWorld(), 1)
	jeep := r.Spawn(pb.ObjectKind_OBJECT_KIND_VEHICLE, "jeep", Server, alice, nil)

	if _, err := r.Claim(jeep.ID, alice, 0); err != nil {
		t.Fatal(err)
	}
	if jeep.Owner != alice {
		t.Fatal("driver owns vehicle")
	}
	if _, err := r.Claim(jeep.ID, bob, 0); err != ErrSeatTaken {
		t.Fatalf("second driver: got %v, want ErrSeatTaken", err)
	}
	if _, err := r.Claim(jeep.ID, bob, 1); err != nil {
		t.Fatal(err)
	}
	if jeep.Owner != alice {
		t.Fatal("passenger does not take ownership")
	}
	if _, err := r.Claim(jeep.ID, bob, 5); err != ErrBadSeat {
		t.Fatalf("got %v, want ErrBadSeat", err)
	}

	final := &pb.ObjectState{Pos: &pb.Vec3{X: 3, Y: 1}, Rot: &pb.Quat{W: 1}, Vel: &pb.Vec3{X: 4}}
	if _, err := r.Release(jeep.ID, alice, final); err != nil {
		t.Fatal(err)
	}
	if jeep.Owner != Server || jeep.State.Pos.X != 3 {
		t.Fatal("driver exit hands vehicle and last state to server")
	}
	if _, err := r.Claim(jeep.ID, bob, 0); err != nil {
		t.Fatal(err)
	}
	if jeep.Owner != bob || jeep.Seats[1] != 0 || jeep.Seats[0] != bob {
		t.Fatalf("bob moves from passenger to driver: seats=%v owner=%d", jeep.Seats, jeep.Owner)
	}
}

func TestOneSeatAtATime(t *testing.T) {
	r := NewRegistry(testWorld(), 1)
	a := r.Spawn(pb.ObjectKind_OBJECT_KIND_VEHICLE, "jeep", Server, alice, nil)
	b := r.Spawn(pb.ObjectKind_OBJECT_KIND_VEHICLE, "jeep", Server, alice, nil)
	r.Claim(a.ID, alice, 0)
	changes, err := r.Claim(b.ID, alice, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(changes) != 2 || a.Owner != Server || a.Seats[0] != 0 || b.Owner != alice {
		t.Fatal("taking a second vehicle releases the first")
	}
}

func TestDisconnectTransfers(t *testing.T) {
	r := NewRegistry(testWorld(), 1)
	me := r.Spawn(pb.ObjectKind_OBJECT_KIND_PLAYER, "soldier", alice, alice, nil)
	jeep := r.Spawn(pb.ObjectKind_OBJECT_KIND_VEHICLE, "jeep", Server, alice, nil)
	r.Claim(jeep.ID, alice, 0)
	r.Claim(jeep.ID, bob, 1)
	epoch := jeep.Epoch

	changes, despawned := r.ReleaseAll(alice)
	if len(despawned) != 1 || despawned[0] != me.ID || r.Get(me.ID) != nil {
		t.Fatal("character removed on disconnect")
	}
	if len(changes) != 1 || jeep.Owner != Server || jeep.Epoch != epoch+1 || jeep.Seats[1] != bob {
		t.Fatal("vehicle goes to server, passenger stays")
	}
}

func TestReassign(t *testing.T) {
	r := NewRegistry(testWorld(), 1)
	jeep := r.Spawn(pb.ObjectKind_OBJECT_KIND_VEHICLE, "jeep", Server, alice, nil)
	r.Claim(jeep.ID, alice, 0)
	if _, err := r.Reassign(jeep.ID, bob); err != nil {
		t.Fatal(err)
	}
	if jeep.Owner != bob || jeep.Seats[0] != 0 {
		t.Fatal("reassign changes owner and clears seats")
	}
	if r.ApplyUpdate(alice, update(jeep, jeep.Epoch, 1)) {
		t.Fatal("old driver is cut off")
	}
}

func TestServerSimMovesAndHandsOff(t *testing.T) {
	r := NewRegistry(testWorld(), 1)
	jeep := r.Spawn(pb.ObjectKind_OBJECT_KIND_VEHICLE, "jeep", Server, alice, nil)
	r.Claim(jeep.ID, alice, 0)
	final := &pb.ObjectState{Pos: &pb.Vec3{X: 0, Y: 3, Z: 0}, Rot: &pb.Quat{W: 1}, Vel: &pb.Vec3{X: 6}}
	r.Release(jeep.ID, alice, final)

	var moved []*Object
	for i := 0; i < 10; i++ {
		moved, _ = r.Step(0.05, 0)
	}
	if len(moved) != 1 || jeep.State.Pos.X <= 0 {
		t.Fatalf("unowned vehicle keeps coasting, x=%v", jeep.State.Pos.X)
	}
	if jeep.State.Pos.Y >= 3 {
		t.Fatal("gravity applies")
	}
	for i := 0; i < 200; i++ {
		r.Step(0.05, 0)
	}
	if !jeep.Asleep {
		t.Fatal("vehicle comes to rest and sleeps")
	}
	if math.Abs(float64(jeep.State.Pos.Y-0.8)) > 1e-3 {
		t.Fatalf("rests on ground at half height, y=%v", jeep.State.Pos.Y)
	}
	if jeep.State.Pos.X+1 > 10+1e-3 && jeep.State.Pos.X-1 < 12 {
		t.Fatal("must not end inside the wall")
	}
	restX, restZ := jeep.State.Pos.X, jeep.State.Pos.Z

	changes, err := r.Claim(jeep.ID, bob, 0)
	if err != nil {
		t.Fatal(err)
	}
	msg := changes[len(changes)-1].Msg()
	if msg.Owner != bob || msg.LastState.Pos.X != restX || msg.LastState.Pos.Z != restZ {
		t.Fatal("new owner receives the server's last state")
	}
	moved, _ = r.Step(0.05, 0)
	if len(moved) != 0 {
		t.Fatal("server stops simulating once a player owns it")
	}
}

func TestWallBlocksServerSim(t *testing.T) {
	r := NewRegistry(testWorld(), 1)
	o := r.Spawn(pb.ObjectKind_OBJECT_KIND_VEHICLE, "jeep", Server, alice,
		&pb.ObjectState{Pos: &pb.Vec3{X: 5, Y: 0.8}, Rot: &pb.Quat{W: 1}, Vel: &pb.Vec3{X: 30}})
	for i := 0; i < 40; i++ {
		r.Step(0.05, 0)
	}
	if o.State.Pos.X+2 > 10.01 {
		t.Fatalf("stopped by wall, x=%v", o.State.Pos.X)
	}
}

func TestAbandonedDespawn(t *testing.T) {
	r := NewRegistry(testWorld(), 1)
	clock := time.Unix(1000, 0)
	r.now = func() time.Time { return clock }
	o := r.Spawn(pb.ObjectKind_OBJECT_KIND_VEHICLE, "jeep", Server, alice, nil)
	clock = clock.Add(4 * time.Minute)
	if _, gone := r.Step(0.05, 5*time.Minute); len(gone) != 0 {
		t.Fatal("not yet")
	}
	clock = clock.Add(2 * time.Minute)
	if _, gone := r.Step(0.05, 5*time.Minute); len(gone) != 1 || gone[0] != o.ID {
		t.Fatal("abandoned vehicle despawns")
	}
}
