//! Raiding: the money machines (droppers, upgraders, conveyor lines) of other
//! teams can be shot down. A line that is destroyed stops paying its owner for
//! `OFFLINE_SECS`, and everyone who helped knock it out earns a bounty.
//!
//! Client-trusted like the rest of the netcode: the shooter reports damage to the
//! base owner's avatar with the weapon `machine:<line>`, the owner's client keeps
//! each line's health, and publishes which lines are offline as a bitmask in the
//! avatar's `extra[4]` so every client can show smoke and stop shooting.

use std::collections::HashMap;

use bevy::prelude::*;
use frontline_proto::pb::{self, ObjectKind, client_msg::Msg as C, server_msg::Msg as S};

use crate::audio::{Play, Snd};
use crate::combat::CombatFx;
use crate::net::{Inbound, Net};
use crate::objects::{Explosion, NetObject, NetObjects, pv};
use crate::particles::{Burst, Emit};
use crate::state::{Economy, Feed, Room};

pub const MAX_HP: f32 = 450.0;
pub const OFFLINE_SECS: f64 = 40.0;
/// Cash for the team that takes a line down.
const BOUNTY: i64 = 350;
const MAX_LINES: usize = 8;
/// Index in the avatar's `extra` that carries the offline bitmask.
pub const X_OFFLINE: usize = 4;

/// A collider belonging to a production line machine.
#[derive(Component)]
pub struct Machine {
    pub owner: u64,
    pub line: usize,
}

/// A hit or blast on something that may be a machine, from the local shooter.
#[derive(Message)]
pub struct MachineStrike {
    /// The collider that was hit directly, if any.
    pub entity: Option<Entity>,
    pub point: Vec3,
    pub damage: f32,
    /// 0 for a direct hit, otherwise the blast radius.
    pub radius: f32,
    pub weapon: String,
}

#[derive(Resource)]
pub struct Machines {
    hp: [f32; MAX_LINES],
    offline_until: [f64; MAX_LINES],
    /// When the local player last shot a given (owner, line).
    shot: HashMap<(u64, usize), f64>,
}

impl Default for Machines {
    fn default() -> Self {
        Self { hp: [MAX_HP; MAX_LINES], offline_until: [0.0; MAX_LINES], shot: HashMap::new() }
    }
}

impl Machines {
    pub fn offline(&self, line: usize, now: f64) -> bool {
        line < MAX_LINES && self.offline_until[line] > now
    }
    fn bits(&self, now: f64) -> f32 {
        (0..MAX_LINES).filter(|&l| self.offline(l, now)).fold(0u32, |a, l| a | (1 << l)) as f32
    }
}

fn bit(obj: &NetObject, line: usize) -> bool {
    (obj.extra.get(X_OFFLINE).copied().unwrap_or(0.0) as u32) >> line & 1 == 1
}

pub fn reset(mut m: ResMut<Machines>) {
    *m = Machines::default();
}

/// Applies strikes: builds one report per (base owner, line) and sends it.
#[allow(clippy::too_many_arguments)]
pub fn strikes(
    mut events: MessageReader<MachineStrike>,
    time: Res<Time>,
    room: Res<Room>,
    net: Res<Net>,
    mut machines: ResMut<Machines>,
    mut fx: ResMut<CombatFx>,
    mut play: MessageWriter<Play>,
    parts: Query<(&Machine, &GlobalTransform)>,
    avatars: Query<&NetObject>,
) {
    let now = time.elapsed_secs_f64();
    // (owner, line) -> (damage, point, weapon)
    let mut totals: HashMap<(u64, usize), (f32, Vec3, String)> = HashMap::new();
    for s in events.read() {
        if s.radius <= 0.0 {
            let Some(e) = s.entity else { continue };
            let Ok((m, _)) = parts.get(e) else { continue };
            let t = totals.entry((m.owner, m.line)).or_insert((0.0, s.point, s.weapon.clone()));
            t.0 += s.damage;
        } else {
            for (m, gt) in &parts {
                let d = (gt.translation().distance(s.point) - 2.0).max(0.0);
                if d >= s.radius {
                    continue;
                }
                let t = totals.entry((m.owner, m.line)).or_insert((0.0, s.point, s.weapon.clone()));
                t.0 = t.0.max(s.damage * (1.0 - d / s.radius));
            }
        }
    }
    for ((owner, line), (damage, point, _)) in totals {
        if owner == room.me || !room.is_enemy(owner) || damage <= 0.0 {
            continue;
        }
        let Some(av) = avatars.iter().find(|o| o.kind == ObjectKind::Player && o.owner == owner) else { continue };
        if bit(av, line) {
            continue;
        }
        net.send(C::Hit(pb::HitReport { target_id: av.id, damage, weapon: format!("machine:{line}"), point: pv(point) }));
        machines.shot.insert((owner, line), now);
        fx.hit_at = now;
        fx.nums.retain(|n| now - n.0 < 1.2);
        fx.nums.push((now, point + Vec3::Y * 0.3, damage, false));
        play.write(Play::ui(Snd::Hit));
    }
}

/// The base owner's side: machine damage arrives as hits on the avatar.
pub fn take_damage(
    mut inbound: MessageReader<Inbound>,
    time: Res<Time>,
    room: Res<Room>,
    mut machines: ResMut<Machines>,
    mut feed: ResMut<Feed>,
    parts: Query<(&Machine, &GlobalTransform)>,
    mut boom: MessageWriter<Explosion>,
) {
    let now = time.elapsed_secs_f64();
    for Inbound(msg) in inbound.read() {
        let S::Hit(h) = msg else { continue };
        let Some(line) = h.weapon.strip_prefix("machine:").and_then(|l| l.parse::<usize>().ok()) else { continue };
        if h.target_id != room.avatar_id || line >= MAX_LINES || !room.is_enemy(h.from) || machines.offline(line, now) {
            continue;
        }
        machines.hp[line] -= h.damage;
        if machines.hp[line] <= 0.0 {
            machines.hp[line] = MAX_HP;
            machines.offline_until[line] = now + OFFLINE_SECS;
            feed.toast(now, format!("Your line {} was knocked offline for {:.0}s!", line + 1, OFFLINE_SECS));
            if let Some((_, gt)) = parts.iter().find(|(m, _)| m.owner == room.me && m.line == line) {
                boom.write(Explosion { pos: gt.translation() + Vec3::Y * 2.0, radius: 5.0 });
            }
        }
    }
}

/// Publishes the offline bitmask on the local avatar.
pub fn publish(time: Res<Time>, room: Res<Room>, machines: Res<Machines>, objs: Res<NetObjects>, mut q: Query<&mut NetObject>) {
    let Some(&e) = objs.0.get(&room.avatar_id) else { return };
    let Ok(mut obj) = q.get_mut(e) else { return };
    if obj.extra.len() <= X_OFFLINE {
        obj.extra.resize(X_OFFLINE + 1, 0.0);
    }
    let bits = machines.bits(time.elapsed_secs_f64());
    if obj.extra[X_OFFLINE] != bits {
        obj.extra[X_OFFLINE] = bits;
    }
}

/// Smoke over offline machines, and the bounty when someone we shot at goes down.
#[allow(clippy::too_many_arguments)]
pub fn effects(
    time: Res<Time>,
    room: Res<Room>,
    machines: ResMut<Machines>,
    mut econ: ResMut<Economy>,
    mut feed: ResMut<Feed>,
    avatars: Query<&NetObject>,
    parts: Query<(&Machine, &GlobalTransform)>,
    mut emit: MessageWriter<Emit>,
    mut play: MessageWriter<Play>,
    mut timer: Local<f32>,
    mut seen: Local<HashMap<(u64, usize), bool>>,
) {
    let now = time.elapsed_secs_f64();
    let mut offline: HashMap<u64, u32> = HashMap::new();
    for o in &avatars {
        if o.kind != ObjectKind::Player {
            continue;
        }
        let bits = if o.owner == room.me { machines.bits(now) as u32 } else { o.extra.get(X_OFFLINE).copied().unwrap_or(0.0) as u32 };
        offline.insert(o.owner, bits);
        if o.owner == room.me {
            continue;
        }
        for line in 0..MAX_LINES {
            let down = bits >> line & 1 == 1;
            let was = seen.insert((o.owner, line), down).unwrap_or(false);
            if down && !was && machines.shot.get(&(o.owner, line)).is_some_and(|t| now - t < 5.0) {
                econ.cash += BOUNTY;
                econ.dirty = true;
                feed.toast(now, format!("Machine destroyed! +${BOUNTY}"));
                play.write(Play::ui(Snd::Capture));
            }
        }
    }
    *timer -= time.delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.4;
    for (m, gt) in &parts {
        if offline.get(&m.owner).is_some_and(|b| b >> m.line & 1 == 1) {
            let p = gt.translation() + Vec3::Y * 1.5;
            emit.write(Emit::new(Burst::Smoke, p, Vec3::Y, 1));
            if rand::random::<f32>() < 0.2 {
                emit.write(Emit::new(Burst::Spark, p, Vec3::Y, 2));
            }
        }
    }
}
