//! Networked objects and ownership on the client.
//!
//! Every object has one owner. Objects this client owns are simulated locally
//! (dynamic Rapier bodies, the character controller) and their state is sent
//! over UDP. Everything else is kinematic and follows interpolated snapshots
//! from its owner or from the server's lightweight sim.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_rapier3d::prelude::*;
use frontline_proto::pb::{self, ObjectKind, server_msg::Msg as S};
use frontline_sim::interp::{Buffer, INTERP_DELAY, Snapshot};

use crate::art::{Build, pal};
use crate::net::{Inbound, Net, UdpIn};
use crate::state::{AppState, GameData, InitialObjects, Room};

#[derive(Component)]
pub struct NetObject {
    pub id: u64,
    pub kind: ObjectKind,
    pub def: String,
    pub owner: u64,
    pub epoch: u32,
    pub seats: Vec<u64>,
    pub spawned_by: u64,
    pub health: f32,
    pub extra: Vec<f32>,
}

impl NetObject {
    pub fn seat_of(&self, uid: u64) -> Option<usize> {
        self.seats.iter().position(|&s| s == uid)
    }
}

/// Snapshot history for objects this client does not own.
#[derive(Component, Default)]
pub struct Interp(pub Buffer);

#[derive(Component)]
pub struct RemoteAvatar;

#[derive(Component)]
pub struct VehicleBody {
    pub class: frontline_sim::defs::VehicleClass,
    pub half: Vec3,
    pub mass: f32,
    pub max_health: f32,
}

/// Body mode currently applied, to detect ownership flips.
#[derive(Component, PartialEq, Eq, Clone, Copy)]
pub enum BodyMode {
    Simulated,
    Following,
}

#[derive(Resource, Default)]
pub struct NetObjects(pub HashMap<u64, Entity>);

/// A ground explosion or vehicle wreck effect request.
#[derive(Message)]
pub struct Explosion {
    pub pos: Vec3,
    pub radius: f32,
}

/// Shots this client fired since the last send.
#[derive(Resource, Default)]
pub struct OutShots(pub Vec<pb::ProjectileFired>);

/// Velocity of the local character, which has no rigid body.
#[derive(Component, Default)]
pub struct MotionVel(pub Vec3);

#[derive(Resource)]
pub struct SendTimer(pub Timer);

pub fn v3(v: &Option<pb::Vec3>) -> Vec3 {
    v.as_ref().map(|v| Vec3::new(v.x, v.y, v.z)).unwrap_or_default()
}
pub fn q4(q: &Option<pb::Quat>) -> Quat {
    q.as_ref().map(|q| Quat::from_xyzw(q.x, q.y, q.z, q.w).normalize()).unwrap_or(Quat::IDENTITY)
}
pub fn pv(v: Vec3) -> Option<pb::Vec3> {
    Some(pb::Vec3 { x: v.x, y: v.y, z: v.z })
}
pub fn pq(q: Quat) -> Option<pb::Quat> {
    Some(pb::Quat { x: q.x, y: q.y, z: q.z, w: q.w })
}

fn kind_of(k: i32) -> ObjectKind {
    ObjectKind::try_from(k).unwrap_or(ObjectKind::Unspecified)
}

/// Spawns the entity for a networked object. The local character is
/// completed by `player::attach_local_player`.
pub fn spawn_object(b: &mut Build, data: &GameData, room: &Room, info: &pb::ObjectInfo, now: f64) -> Entity {
    let st = info.state.clone().unwrap_or_default();
    let pos = v3(&st.pos);
    let rot = q4(&st.rot);
    let kind = kind_of(info.kind);
    let local = info.owner == room.me;
    let obj = NetObject {
        id: info.id,
        kind,
        def: info.def.clone(),
        owner: info.owner,
        epoch: info.epoch,
        seats: info.seats.clone(),
        spawned_by: info.spawned_by,
        health: st.health,
        extra: st.extra.clone(),
    };
    let mut buf = Buffer::default();
    buf.push(Snapshot { t: now, pos, rot, vel: v3(&st.vel) });
    let e = b
        .commands
        .spawn((
            Transform::from_translation(pos).with_rotation(rot),
            Visibility::default(),
            obj,
            Interp(buf),
            DespawnOnExit(AppState::InGame),
        ))
        .id();

    match kind {
        ObjectKind::Player => {
            let team = room.team_of(info.owner).unwrap_or(0);
            let color = room.settings.teams.get(team as usize).map(|t| t.color).unwrap_or(pal::KHAKI);
            b.soldier(e, color);
            b.commands.entity(e).insert(crate::art::Gait::default());
            if local {
                b.commands.entity(e).insert(MotionVel::default());
            } else {
                b.commands.entity(e).insert((
                    RemoteAvatar,
                    RigidBody::KinematicPositionBased,
                    Collider::capsule_y(0.5, 0.4),
                ));
            }
        }
        _ => {
            let def = data.vehicles.get(&info.def).cloned().unwrap_or(frontline_sim::defs::VehicleDef {
                class: frontline_sim::defs::VehicleClass::Prop,
                seats: 0,
                half_extents: [0.5; 3],
                mass: 50.0,
                health: 100.0,
            });
            let he = Vec3::from(def.half_extents);
            let accent = room
                .team_of(info.spawned_by)
                .and_then(|t| room.settings.teams.get(t as usize))
                .map(|t| t.color)
                .unwrap_or(pal::HAZARD);
            b.vehicle(e, &info.def, he, accent);
            b.commands.entity(e).insert((
                VehicleBody { class: def.class, half: he, mass: def.mass, max_health: def.health },
                Collider::cuboid(he.x, he.y, he.z),
                ColliderMassProperties::Mass(def.mass),
                Velocity { linear: v3(&st.vel), angular: v3(&st.angvel) },
                Ccd::enabled(),
                RigidBody::KinematicPositionBased,
                BodyMode::Following,
            ));
        }
    }
    e
}

pub fn spawn_initial(
    mut b: Build,
    initial: Res<InitialObjects>,
    data: Res<GameData>,
    room: Res<Room>,
    mut objs: ResMut<NetObjects>,
    time: Res<Time>,
) {
    objs.0.clear();
    let now = time.elapsed_secs_f64();
    for info in &initial.0 {
        let e = spawn_object(&mut b, &data, &room, info, now);
        objs.0.insert(info.id, e);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn handle_object_messages(
    mut inbound: MessageReader<Inbound>,
    mut b: Build,
    data: Res<GameData>,
    room: Res<Room>,
    mut objs: ResMut<NetObjects>,
    mut q: Query<(&mut NetObject, &mut Interp, &mut Transform, Option<&mut Velocity>)>,
    mut boom: MessageWriter<Explosion>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for Inbound(msg) in inbound.read() {
        match msg {
            S::ObjectSpawned(s) => {
                let Some(info) = &s.object else { continue };
                if objs.0.contains_key(&info.id) {
                    continue;
                }
                let e = spawn_object(&mut b, &data, &room, info, now);
                objs.0.insert(info.id, e);
            }
            S::ObjectDespawned(d) => {
                if let Some(e) = objs.0.remove(&d.id) {
                    if d.destroyed {
                        if let Ok((_, _, t, _)) = q.get(e) {
                            boom.write(Explosion { pos: t.translation, radius: 6.0 });
                        }
                    }
                    b.commands.entity(e).despawn();
                }
            }
            S::Ownership(o) => {
                let Some(&e) = objs.0.get(&o.id) else { continue };
                let Ok((mut obj, mut interp, mut t, vel)) = q.get_mut(e) else { continue };
                let st = o.last_state.clone().unwrap_or_default();
                let was_mine = obj.owner == room.me;
                obj.owner = o.owner;
                obj.epoch = o.epoch;
                obj.seats = o.seats.clone();
                let mine = obj.owner == room.me;
                if mine && !was_mine {
                    // Take over from the last known state so physics continues smoothly.
                    t.translation = v3(&st.pos);
                    t.rotation = q4(&st.rot);
                    if let Some(mut v) = vel {
                        v.linear = v3(&st.vel);
                        v.angular = v3(&st.angvel);
                    }
                    obj.health = st.health;
                } else if !mine && was_mine {
                    interp.0.clear();
                    interp.0.push(Snapshot { t: now, pos: t.translation, rot: t.rotation, vel: v3(&st.vel) });
                }
            }
            _ => {}
        }
    }
}

/// Applies relayed state to objects owned by someone else.
pub fn apply_udp(
    mut packets: MessageReader<UdpIn>,
    objs: Res<NetObjects>,
    room: Res<Room>,
    mut q: Query<(&mut NetObject, &mut Interp)>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for UdpIn(p) in packets.read() {
        for u in &p.updates {
            let Some(&e) = objs.0.get(&u.id) else { continue };
            let Ok((mut obj, mut interp)) = q.get_mut(e) else { continue };
            if obj.owner == room.me || u.epoch < obj.epoch {
                continue; // we own it, or the packet predates the last ownership change
            }
            let Some(st) = &u.state else { continue };
            interp.0.push(Snapshot { t: now, pos: v3(&st.pos), rot: q4(&st.rot), vel: v3(&st.vel) });
            obj.health = st.health;
            obj.extra = st.extra.clone();
        }
    }
}

/// Switches bodies between simulated (owned here) and following (owned elsewhere).
pub fn sync_body_modes(
    mut commands: Commands,
    room: Res<Room>,
    mut q: Query<(Entity, &NetObject, &mut BodyMode, &VehicleBody), Changed<NetObject>>,
) {
    for (e, obj, mut mode, vb) in &mut q {
        let want = if obj.owner == room.me { BodyMode::Simulated } else { BodyMode::Following };
        if *mode == want {
            continue;
        }
        *mode = want;
        match want {
            BodyMode::Simulated => {
                commands.entity(e).insert(RigidBody::Dynamic);
                crate::vehicles::attach_controller(&mut commands, e, &obj.def, vb);
            }
            BodyMode::Following => {
                commands.entity(e).insert(RigidBody::KinematicPositionBased);
                crate::vehicles::detach_controller(&mut commands, e);
            }
        }
    }
}

pub fn interpolate(room: Res<Room>, time: Res<Time>, mut q: Query<(&NetObject, &Interp, &mut Transform)>) {
    let t = time.elapsed_secs_f64() - INTERP_DELAY;
    for (obj, interp, mut tr) in &mut q {
        if obj.owner == room.me {
            continue;
        }
        if let Some((p, r)) = interp.0.sample(t) {
            tr.translation = p;
            tr.rotation = r;
        }
    }
}

/// Hides remote soldiers that are seated in vehicles or dead.
pub fn avatar_visuals(room: Res<Room>, q: Query<(&NetObject, &Children), With<RemoteAvatar>>, mut vis: Query<&mut Visibility>) {
    for (obj, children) in &q {
        if obj.owner == room.me {
            continue;
        }
        let seated = obj.extra.get(2).copied().unwrap_or(0.0) > 0.5;
        let dead = obj.health <= 0.0;
        for c in children.iter() {
            if let Ok(mut v) = vis.get_mut(c) {
                let show = !(seated || dead);
                *v = if show { Visibility::Inherited } else { Visibility::Hidden };
            }
        }
    }
}

/// Sends the state of everything this client owns, at the room tick rate.
pub fn send_owned(
    time: Res<Time>,
    mut timer: ResMut<SendTimer>,
    mut net: ResMut<Net>,
    room: Res<Room>,
    mut shots: ResMut<OutShots>,
    q: Query<(&NetObject, &Transform, Option<&Velocity>, Option<&MotionVel>)>,
) {
    timer.0.set_duration(std::time::Duration::from_secs_f32(1.0 / room.tick_rate as f32));
    if !timer.0.tick(time.delta()).just_finished() && shots.0.is_empty() {
        return;
    }
    let mut pkt = pb::UdpPacket { shots: std::mem::take(&mut shots.0), ..default() };
    for (obj, t, vel, mv) in &q {
        if obj.owner != room.me {
            continue;
        }
        let (lin, ang) = match (vel, mv) {
            (Some(v), _) => (v.linear, v.angular),
            (None, Some(m)) => (m.0, Vec3::ZERO),
            _ => (Vec3::ZERO, Vec3::ZERO),
        };
        pkt.updates.push(pb::ObjectUpdate {
            id: obj.id,
            epoch: obj.epoch,
            state: Some(pb::ObjectState {
                pos: pv(t.translation),
                rot: pq(t.rotation),
                vel: pv(lin),
                angvel: pv(ang),
                health: obj.health,
                extra: obj.extra.clone(),
            }),
        });
    }
    if !pkt.updates.is_empty() || !pkt.shots.is_empty() {
        net.send_udp(pkt);
    }
}

/// Current state of an object, for handing it back to the server.
pub fn state_of(obj: &NetObject, t: &Transform, vel: Option<&Velocity>) -> pb::ObjectState {
    pb::ObjectState {
        pos: pv(t.translation),
        rot: pq(t.rotation),
        vel: pv(vel.map(|v| v.linear).unwrap_or_default()),
        angvel: pv(vel.map(|v| v.angular).unwrap_or_default()),
        health: obj.health,
        extra: obj.extra.clone(),
    }
}
