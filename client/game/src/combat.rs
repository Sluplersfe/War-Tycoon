//! Weapons, projectiles, hit reporting and damage intake.
//!
//! The shooter's client decides hits and reports them to the server, which
//! forwards each report to whoever owns the target. The owner applies the
//! damage (player avatar, or a vehicle the owner is driving); targets owned
//! by the server are damaged by the server itself.

use std::collections::HashMap;

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy_rapier3d::prelude::*;
use frontline_proto::pb::{self, ObjectKind, client_msg::Msg as C, server_msg::Msg as S};
use frontline_sim::defs::VehicleClass;
use frontline_sim::weapons::{WeaponDef, WeaponKind, WeaponState, splash_damage};

use crate::art::{Build, pal};
use crate::net::{Inbound, Net, UdpIn};
use crate::objects::{Explosion, NetObject, NetObjects, OutShots, VehicleBody, pv, v3};
use crate::guns::{self, Armory, Handling};
use crate::player::{Controls, Look, LocalPlayer, Seat, Vitals};
use crate::viewmodel::Vm;
use crate::state::{Economy, GameData, Room};
use crate::world::MainCamera;

const BASE_WEAPONS: [&str; 3] = ["knife", "pistol", "grenade"];

#[derive(Resource, Default)]
pub struct Loadout {
    owned: Vec<String>,
    states: Vec<WeaponState>,
    /// Definitions after attachments, parallel to `owned`.
    defs: Vec<WeaponDef>,
    handling: Vec<Handling>,
    pub current: usize,
    mounted: HashMap<String, WeaponState>,
}

impl Loadout {
    pub fn current_id(&self) -> &str {
        self.owned.get(self.current).map(String::as_str).unwrap_or("knife")
    }

    pub fn owned(&self) -> &[String] {
        &self.owned
    }

    pub fn handling(&self) -> Handling {
        self.handling.get(self.current).copied().unwrap_or_default()
    }

    pub fn current_ammo(&self) -> u32 {
        self.states.get(self.current).map(|s| s.ammo).unwrap_or(0)
    }

    /// 0..1 through the current reload, if one is running.
    pub fn reload_progress(&self) -> Option<f32> {
        let (d, s) = (self.defs.get(self.current)?, self.states.get(self.current)?);
        (s.reloading > 0.0 && d.reload > 0.0).then(|| (1.0 - s.reloading / d.reload).clamp(0.0, 1.0))
    }

    /// Resets to full magazines (on spawn).
    pub fn refill(&mut self) {
        for (d, st) in self.defs.iter().zip(self.states.iter_mut()) {
            *st = WeaponState::new(d);
        }
        self.mounted.clear();
    }

    fn add(&mut self, data: &GameData, armory: &Armory, id: &str) {
        if self.owned.iter().any(|o| o == id) {
            return;
        }
        let Some(def) = data.weapon(id) else { return };
        let keep = self.owned.get(self.current).cloned();
        self.owned.push(id.to_string());
        self.states.push(WeaponState::new(&guns::apply(def, &armory.get(id)).0));
        // Keep the list ordered by slot.
        let mut order: Vec<usize> = (0..self.owned.len()).collect();
        order.sort_by_key(|&i| data.weapon(&self.owned[i]).map(|w| w.slot).unwrap_or(99));
        self.owned = order.iter().map(|&i| self.owned[i].clone()).collect();
        let states: Vec<WeaponState> = order.iter().map(|&i| self.states[i].clone()).collect();
        self.states = states;
        if let Some(k) = keep {
            self.current = self.owned.iter().position(|o| *o == k).unwrap_or(0);
        }
    }

    /// Recomputes attachment-adjusted definitions.
    fn rebuild(&mut self, data: &GameData, armory: &Armory) {
        self.defs.clear();
        self.handling.clear();
        for (id, st) in self.owned.iter().zip(self.states.iter_mut()) {
            let def = data.weapon(id).expect("owned weapons exist");
            let (d, h) = guns::apply(def, &armory.get(id));
            st.ammo = st.ammo.min(d.mag);
            self.defs.push(d);
            self.handling.push(h);
        }
    }

    /// Weapons granted by owned racks, plus the starting kit.
    pub fn sync(&mut self, data: &GameData, econ: &Economy, armory: &Armory) {
        let mut allowed: Vec<String> = BASE_WEAPONS.iter().map(|s| s.to_string()).collect();
        for item in data.catalog.owns_kind(&econ.base) {
            if let frontline_sim::tycoon::ItemKind::WeaponRack { weapon } = &item.kind {
                allowed.push(weapon.clone());
            }
        }
        let keep = self.owned.get(self.current).cloned();
        let mut i = 0;
        while i < self.owned.len() {
            if allowed.contains(&self.owned[i]) {
                i += 1;
            } else {
                self.owned.remove(i);
                self.states.remove(i);
            }
        }
        for id in &allowed {
            self.add(data, armory, id);
        }
        self.current = keep.and_then(|k| self.owned.iter().position(|o| *o == k)).unwrap_or(0).min(self.owned.len().saturating_sub(1));
        self.rebuild(data, armory);
    }

    fn state_mut<'a>(&'a mut self, data: &'a GameData, id: &str, mounted: bool) -> Option<(&'a WeaponDef, &'a mut WeaponState)> {
        if mounted {
            let def = data.weapon(id)?;
            Some((def, self.mounted.entry(id.to_string()).or_insert_with(|| WeaponState::new(def))))
        } else {
            let i = self.owned.iter().position(|o| o == id)?;
            Some((self.defs.get(i)?, self.states.get_mut(i)?))
        }
    }

    /// (name, ammo, magazine, reloading) of what the player would fire now.
    pub fn hud(&self, data: &GameData, seat: &Seat) -> Option<(String, u32, u32, bool)> {
        if let Some(id) = mounted_weapon(seat) {
            let def = data.weapon(id)?;
            let st = self.mounted.get(id).cloned().unwrap_or_else(|| WeaponState::new(def));
            return Some((def.name.clone(), st.ammo, def.mag, st.reloading > 0.0));
        }
        let (d, st) = (self.defs.get(self.current)?, self.states.get(self.current)?);
        Some((d.name.clone(), st.ammo, d.mag, st.reloading > 0.0))
    }
}

/// The vehicle-mounted weapon the current seat operates, if any.
pub fn mounted_weapon(seat: &Seat) -> Option<&'static str> {
    let s = seat.0?;
    match (s.class, s.index) {
        (VehicleClass::Tank, 0) => Some("tank_cannon"),
        (VehicleClass::Tank, _) => Some("mounted_gun"),
        (VehicleClass::Heli, 0) => Some("mounted_gun"),
        (VehicleClass::Plane, 0) => Some("plane_gun"),
        _ => None,
    }
}

#[derive(Resource, Default)]
pub struct CombatFx {
    /// When the local player last fired a hand weapon (drives the view model).
    pub fired_at: f64,
    pub hit_at: f64,
    pub hurt_at: f64,
    pub hurt_from: Option<Vec3>,
}

/// Short-lived effect entity: despawns after `life`, scaling by `grow` per second.
#[derive(Component)]
pub struct Fx {
    pub life: f32,
    pub grow: Vec3,
}

#[derive(Component)]
pub struct Projectile {
    pub vel: Vec3,
    pub weapon: String,
    pub mine: bool,
    pub life: f32,
}

pub fn reset_loadout(mut loadout: ResMut<Loadout>, data: Res<GameData>, econ: Res<Economy>, armory: Res<Armory>) {
    *loadout = Loadout::default();
    loadout.sync(&data, &econ, &armory);
}

pub fn sync_racks(mut loadout: ResMut<Loadout>, data: Res<GameData>, econ: Res<Economy>, armory: Res<Armory>) {
    if econ.is_changed() || armory.is_changed() {
        loadout.sync(&data, &econ, &armory);
    }
}

pub fn tick_weapons(time: Res<Time>, data: Res<GameData>, mut loadout: ResMut<Loadout>) {
    let dt = time.delta_secs();
    let Loadout { defs, states, mounted, .. } = &mut *loadout;
    for (def, st) in defs.iter().zip(states.iter_mut()) {
        st.tick(def, dt);
    }
    for (id, st) in mounted.iter_mut() {
        if let Some(def) = data.weapon(id) {
            st.tick(def, dt);
        }
    }
}

pub fn select_weapon(
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    controls: Res<Controls>,
    seat: Res<Seat>,
    data: Res<GameData>,
    mut loadout: ResMut<Loadout>,
) {
    if !controls.captured {
        return;
    }
    let digits = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    for (i, k) in digits.iter().enumerate() {
        if keys.just_pressed(*k) && i < loadout.owned.len() {
            loadout.current = i;
        }
    }
    let n = loadout.owned.len().max(1);
    if scroll.delta.y > 0.0 {
        loadout.current = (loadout.current + n - 1) % n;
    } else if scroll.delta.y < 0.0 {
        loadout.current = (loadout.current + 1) % n;
    }
    if keys.just_pressed(KeyCode::KeyR) && mounted_weapon(&seat).is_none() {
        let id = loadout.current_id().to_string();
        if let Some((def, st)) = loadout.state_mut(&data, &id, false) {
            st.start_reload(def);
        }
    }
}

fn random_cone(dir: Vec3, spread: f32) -> Vec3 {
    if spread <= 0.0 {
        return dir;
    }
    let up = if dir.y.abs() > 0.95 { Vec3::X } else { Vec3::Y };
    let right = dir.cross(up).normalize();
    let up = right.cross(dir);
    let a = spread * rand::random::<f32>().sqrt();
    let phi = rand::random::<f32>() * std::f32::consts::TAU;
    (dir + right * (a.tan() * phi.cos()) + up * (a.tan() * phi.sin())).normalize()
}

/// Reports damage to a hit target if it is a valid enemy. Returns true if it counts as a hit.
pub fn target_hit(obj: &NetObject, t: &Transform, point: Vec3, room: &Room) -> Option<f32> {
    match obj.kind {
        ObjectKind::Player => {
            if obj.owner == room.me || obj.health <= 0.0 || !room.is_enemy(obj.owner) {
                return None;
            }
            let head = point.y - t.translation.y > 0.55;
            Some(if head { 1.5 } else { 1.0 })
        }
        ObjectKind::Vehicle | ObjectKind::Prop => {
            if obj.spawned_by == room.me || (obj.spawned_by != 0 && !room.is_enemy(obj.spawned_by)) {
                return None;
            }
            Some(1.0)
        }
        _ => None,
    }
}

pub fn spawn_tracer(b: &mut Build, from: Vec3, to: Vec3) {
    let d = to - from;
    let len = d.length();
    if len < 0.5 {
        return;
    }
    let mat = b.glow(pal::AMBER);
    let mesh = b.art.cube.clone();
    b.commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(from + d / 2.0).looking_to(d / len, Vec3::Y).with_scale(Vec3::new(0.05, 0.05, len)),
        Fx { life: 0.07, grow: Vec3::ZERO },
        bevy::light::NotShadowCaster,
    ));
}

fn spawn_spark(b: &mut Build, at: Vec3) {
    let mat = b.glow(pal::AMBER);
    let mesh = b.art.sphere.clone();
    b.commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(at).with_scale(Vec3::splat(0.25)),
        Fx { life: 0.12, grow: Vec3::splat(-1.5) },
        bevy::light::NotShadowCaster,
    ));
}

fn spawn_projectile(b: &mut Build, at: Vec3, vel: Vec3, weapon: &str, mine: bool) {
    let big = weapon == "rocket" || weapon == "tank_cannon";
    let (color, size) = if weapon == "grenade" { (pal::OLIVE_DARK, 0.3) } else { (pal::HAZARD, if big { 0.45 } else { 0.3 }) };
    let mat = if weapon == "grenade" { b.mat(color) } else { b.glow(color) };
    let mesh = b.art.sphere.clone();
    b.commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(at).with_scale(Vec3::splat(size)),
        Projectile { vel, weapon: weapon.to_string(), mine, life: 8.0 },
        DespawnOnExit(crate::state::AppState::InGame),
    ));
}

#[allow(clippy::too_many_arguments)]
pub fn fire(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    (controls, vitals, seat, room, data, net, vm): (Res<Controls>, Res<Vitals>, Res<Seat>, Res<Room>, Res<GameData>, Res<Net>, Res<Vm>),
    mut loadout: ResMut<Loadout>,
    mut look: ResMut<Look>,
    mut shots: ResMut<OutShots>,
    mut fx: ResMut<CombatFx>,
    mut b: Build,
    rapier: ReadRapierContext,
    cam: Query<&Transform, With<MainCamera>>,
    me: Query<Entity, With<LocalPlayer>>,
    vehicles: Query<(&Transform, &NetObject), With<VehicleBody>>,
    targets: Query<(&NetObject, &Transform), Without<MainCamera>>,
) {
    if !controls.captured || !vitals.alive() {
        return;
    }
    let (Ok(cam), Ok(pe)) = (cam.single(), me.single()) else { return };
    let Ok(ctx) = rapier.single() else { return };
    let mounted = mounted_weapon(&seat);
    let wid = mounted.unwrap_or_else(|| loadout.current_id()).to_string();
    let Some(base) = data.weapon(&wid) else { return };
    let wants = if base.auto { mouse.pressed(MouseButton::Left) } else { mouse.just_pressed(MouseButton::Left) };
    if !wants {
        return;
    }
    let handling = if mounted.is_some() { Handling::default() } else { loadout.handling() };
    let Some((def, st)) = loadout.state_mut(&data, &wid, mounted.is_some()) else { return };
    if !st.try_fire(def) {
        return;
    }
    let def = def.clone();

    let now = time.elapsed_secs_f64();
    let mut exclude_filter = QueryFilter::default().exclude_collider(pe).exclude_sensors();
    if let Some(s) = seat.0 {
        exclude_filter = exclude_filter.exclude_collider(s.vehicle).exclude_rigid_body(s.vehicle);
    }

    // Aim point from the crosshair.
    let cdir = cam.forward().as_vec3();
    let reach = match def.kind {
        WeaponKind::Hitscan { range, .. } => range.min(700.0),
        _ => 400.0,
    };
    let aim = ctx
        .cast_ray(cam.translation, cdir, reach + 6.0, true, exclude_filter)
        .map(|(_, toi)| cam.translation + cdir * toi)
        .unwrap_or(cam.translation + cdir * reach);

    // Ray origin and direction; on foot, shots leave the eye and tracers the view-model muzzle.
    let on_foot = mounted.is_none();
    let (muzzle, mut dir) = match (seat.0, mounted) {
        (Some(s), Some(_)) => match vehicles.get(s.vehicle) {
            Ok((vt, vo)) => {
                let (o, d) = crate::vehicles::muzzle(s.class, vt, &vo.extra, (aim - vt.translation).normalize_or_zero());
                let d = if s.class == VehicleClass::Plane { vt.rotation * Vec3::Z } else { d };
                // Passengers on the tank's mounted gun fire from the hatch toward the crosshair.
                if s.class == VehicleClass::Tank && s.index > 0 {
                    let o = vt.translation + vt.rotation * Vec3::new(0.0, 2.2, -0.6);
                    (o, (aim - o).normalize_or_zero())
                } else {
                    (o, d)
                }
            }
            Err(_) => (cam.translation, cdir),
        },
        _ => match def.kind {
            WeaponKind::Projectile { .. } => {
                let m = vm.muzzle_world;
                let d = (aim - m).normalize_or_zero();
                if m.distance(cam.translation) < 2.0 && d.dot(cdir) > 0.5 { (m, d) } else { (cam.translation + cdir * 0.5, cdir) }
            }
            _ => (cam.translation, cdir),
        },
    };
    if dir == Vec3::ZERO {
        dir = cdir;
    }
    let tracer_from = if on_foot && vm.muzzle_world.distance(cam.translation) < 2.0 { vm.muzzle_world } else { muzzle };

    // Aiming down sights tightens the cone; the marksman rifle is wild from the hip.
    let ads = if on_foot { look.ads } else { 0.0 };
    let spread_mult = handling.hip + (0.3 - handling.hip) * ads;
    let extra_spread = if on_foot && wid == "sniper" { 0.06 * (1.0 - ads) } else { 0.0 };

    let mut damage_by_target: HashMap<u64, (f32, Vec3)> = HashMap::new();
    match def.kind {
        WeaponKind::Melee { range } => {
            if let Some((e, toi)) = ctx.cast_ray(muzzle, dir, range, true, exclude_filter) {
                let point = muzzle + dir * toi;
                if let Ok((obj, t)) = targets.get(e) {
                    if let Some(mult) = target_hit(obj, t, point, &room) {
                        let entry = damage_by_target.entry(obj.id).or_insert((0.0, point));
                        entry.0 += def.damage * mult;
                    }
                }
            }
        }
        WeaponKind::Hitscan { range, spread, pellets } => {
            for _ in 0..pellets.max(1) {
                let d = random_cone(dir, spread * spread_mult + extra_spread);
                let hit = ctx.cast_ray(muzzle, d, range, true, exclude_filter);
                let end = muzzle + d * hit.map(|h| h.1).unwrap_or(range.min(300.0));
                spawn_tracer(&mut b, tracer_from, end);
                if let Some((e, _)) = hit {
                    match targets.get(e) {
                        Ok((obj, t)) => {
                            if let Some(mult) = target_hit(obj, t, end, &room) {
                                let entry = damage_by_target.entry(obj.id).or_insert((0.0, end));
                                entry.0 += def.damage * mult;
                            }
                        }
                        Err(_) => spawn_spark(&mut b, end),
                    }
                }
            }
            shots.0.push(pb::ProjectileFired { shooter: 0, weapon: wid.clone(), origin: pv(muzzle), dir: pv(dir), seed: 0 });
        }
        WeaponKind::Projectile { speed, .. } => {
            spawn_projectile(&mut b, muzzle, dir * speed, &wid, true);
            shots.0.push(pb::ProjectileFired { shooter: 0, weapon: wid.clone(), origin: pv(muzzle), dir: pv(dir), seed: 0 });
        }
    }
    if on_foot {
        fx.fired_at = now;
        // Camera kick: mostly up, a little sideways; steadier when aiming.
        let kick = handling.recoil.to_radians() * (1.0 - 0.35 * ads) * (0.8 + 0.4 * rand::random::<f32>());
        look.pitch = (look.pitch + kick).min(1.3);
        look.yaw += (rand::random::<f32>() - 0.5) * kick * 0.6;
    }
    for (id, (damage, point)) in damage_by_target {
        net.send(C::Hit(pb::HitReport { target_id: id, damage, weapon: wid.clone(), point: pv(point) }));
        fx.hit_at = now;
    }
}

/// Moves projectiles and detonates them on impact.
#[allow(clippy::too_many_arguments)]
pub fn update_projectiles(
    time: Res<Time>,
    mut commands: Commands,
    data: Res<GameData>,
    room: Res<Room>,
    net: Res<Net>,
    mut vitals: ResMut<Vitals>,
    mut fx: ResMut<CombatFx>,
    rapier: ReadRapierContext,
    mut q: Query<(Entity, &mut Transform, &mut Projectile)>,
    me: Query<(Entity, &Transform), (With<LocalPlayer>, Without<Projectile>)>,
    targets: Query<(&NetObject, &Transform), Without<Projectile>>,
    mut boom: MessageWriter<Explosion>,
) {
    let Ok(ctx) = rapier.single() else { return };
    let dt = time.delta_secs().min(0.05);
    let now = time.elapsed_secs_f64();
    let shooter = me.single().ok();
    for (e, mut t, mut p) in &mut q {
        p.life -= dt;
        let Some(def) = data.weapon(&p.weapon) else {
            commands.entity(e).despawn();
            continue;
        };
        let WeaponKind::Projectile { gravity, splash, .. } = def.kind else {
            commands.entity(e).despawn();
            continue;
        };
        let from = t.translation;
        p.vel.y -= gravity * dt;
        let step = p.vel * dt;
        let len = step.length();
        let mut filter = QueryFilter::default().exclude_sensors();
        if p.mine {
            if let Some((pe, _)) = shooter {
                filter = filter.exclude_collider(pe);
            }
        }
        let hit = if len > 1e-4 { ctx.cast_ray(from, step / len, len, true, filter) } else { None };
        if let Some((_, toi)) = hit {
            let point = from + step / len * toi;
            boom.write(Explosion { pos: point, radius: splash });
            if p.mine {
                let mut sent = Vec::new();
                for (obj, ot) in &targets {
                    let d = if obj.kind == ObjectKind::Player {
                        ot.translation.distance(point)
                    } else {
                        (ot.translation.distance(point) - 1.5).max(0.0)
                    };
                    let dmg = splash_damage(def, d);
                    if dmg <= 0.0 {
                        continue;
                    }
                    if target_hit(obj, ot, point, &room).is_some() {
                        sent.push(obj.id);
                        net.send(C::Hit(pb::HitReport { target_id: obj.id, damage: dmg, weapon: p.weapon.clone(), point: pv(point) }));
                    }
                }
                if !sent.is_empty() {
                    fx.hit_at = now;
                }
                if let Some((_, pt)) = shooter {
                    let dmg = splash_damage(def, pt.translation.distance(point)) * 0.5;
                    if dmg > 0.0 && vitals.damage(now, dmg, 0, &p.weapon) {
                        fx.hurt_at = now;
                    }
                }
            }
            commands.entity(e).despawn();
        } else {
            t.translation = from + step;
            if p.life <= 0.0 {
                commands.entity(e).despawn();
            }
        }
    }
}

/// Draws other players' shots from the relayed UDP shot list.
pub fn remote_shots(
    mut packets: MessageReader<UdpIn>,
    room: Res<Room>,
    data: Res<GameData>,
    mut b: Build,
    rapier: ReadRapierContext,
) {
    let Ok(ctx) = rapier.single() else { return };
    for UdpIn(p) in packets.read() {
        for s in &p.shots {
            if s.shooter == room.me {
                continue;
            }
            let Some(def) = data.weapon(&s.weapon) else { continue };
            let origin = v3(&s.origin);
            let dir = v3(&s.dir).normalize_or_zero();
            if dir == Vec3::ZERO {
                continue;
            }
            match def.kind {
                WeaponKind::Hitscan { range, .. } => {
                    let hit = ctx.cast_ray(origin, dir, range, true, QueryFilter::default().exclude_sensors());
                    let end = origin + dir * hit.map(|h| h.1).unwrap_or(range.min(300.0));
                    spawn_tracer(&mut b, origin, end);
                }
                WeaponKind::Projectile { speed, .. } => spawn_projectile(&mut b, origin, dir * speed, &s.weapon, false),
                WeaponKind::Melee { .. } => {}
            }
        }
    }
}

/// Damage reports addressed to targets this client owns.
#[allow(clippy::too_many_arguments)]
pub fn handle_hits(
    mut inbound: MessageReader<Inbound>,
    time: Res<Time>,
    room: Res<Room>,
    objs: Res<NetObjects>,
    net: Res<Net>,
    mut vitals: ResMut<Vitals>,
    mut fx: ResMut<CombatFx>,
    mut q: Query<(&mut NetObject, &Transform)>,
) {
    let now = time.elapsed_secs_f64();
    for Inbound(msg) in inbound.read() {
        let S::Hit(h) = msg else { continue };
        if h.target_id == room.avatar_id {
            if room.is_enemy(h.from) && vitals.damage(now, h.damage, h.from, &h.weapon) {
                fx.hurt_at = now;
                fx.hurt_from = h.point.as_ref().map(|p| Vec3::new(p.x, p.y, p.z));
            }
            continue;
        }
        let Some(&e) = objs.0.get(&h.target_id) else { continue };
        let Ok((mut obj, _)) = q.get_mut(e) else { continue };
        if obj.owner != room.me {
            continue;
        }
        obj.health -= h.damage;
        if obj.health <= 0.0 {
            net.send(C::Despawn(pb::DespawnObject { id: obj.id, destroyed: true }));
        }
    }
}

pub fn spawn_explosions(mut events: MessageReader<Explosion>, mut b: Build) {
    for ev in events.read() {
        let mat = b.glow(pal::HAZARD);
        let mesh = b.art.sphere.clone();
        let life = 0.45;
        b.commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(mat),
            Transform::from_translation(ev.pos).with_scale(Vec3::splat(1.0)),
            Fx { life, grow: Vec3::splat((ev.radius * 2.0 - 1.0).max(1.0) / life) },
            bevy::light::NotShadowCaster,
        ));
        let smoke = b.mat(pal::BLACK);
        let mesh = b.art.sphere.clone();
        b.commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(smoke),
            Transform::from_translation(ev.pos + Vec3::Y * ev.radius * 0.4).with_scale(Vec3::splat(0.5)),
            Fx { life: 1.0, grow: Vec3::splat(ev.radius * 1.2) },
            bevy::light::NotShadowCaster,
        ));
    }
}

pub fn update_fx(time: Res<Time>, mut commands: Commands, mut q: Query<(Entity, &mut Fx, &mut Transform)>) {
    let dt = time.delta_secs();
    for (e, mut f, mut t) in &mut q {
        f.life -= dt;
        if f.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        if f.grow != Vec3::ZERO {
            t.scale = (t.scale + f.grow * dt).max(Vec3::splat(0.01));
        }
    }
}
