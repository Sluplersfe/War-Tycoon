//! The tycoon: base models for every player in the room, buy pads, ore
//! production, the claim pad, base defences and support buildings.
//!
//! Bases are plot-local scenes built from the shared item catalogue. Only the
//! local player's own base is simulated (ore, income, turrets, healing); other
//! bases are static models rebuilt when their owner's saved state changes.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_rapier3d::prelude::*;
use frontline_proto::pb::{self, ObjectKind, client_msg::Msg as C};
use frontline_sim::tycoon::{BaseState, ItemDef, ItemKind, can_rebirth, purchase_xp, rebirth_multiplier, rebirth_price};

use crate::art::{Build, pal};
use crate::combat::spawn_tracer;
use crate::net::Net;
use crate::objects::{NetObject, OutShots, VehicleBody, pq, pv};
use crate::player::{Action, Interact, LocalPlayer, Seat, Vitals};
use crate::state::{AppState, Economy, Feed, GameData, Room};

#[derive(Message)]
pub struct RebirthRequest;

#[derive(Message)]
pub struct SpawnVehicleRequest {
    pub def: String,
    pub pos: Vec3,
    pub yaw: f32,
}

#[derive(Component)]
pub struct BaseRoot {
    pub user: u64,
    pub plot: usize,
    pub sig: String,
    pub content: Entity,
}

#[derive(Component)]
pub struct BuyPad {
    pub item: String,
    pub local: Vec3,
}

#[derive(Component)]
pub struct Ore {
    pub value: f32,
    pub line: usize,
    pub vy: f32,
    pub upgrades: u8,
}

/// Rotating head of a base turret.
#[derive(Component)]
pub struct BaseTurret {
    pub user: u64,
    pub plot_yaw: f32,
    pub damage: f32,
    pub range: f32,
    pub rate: f32,
    pub cooldown: f32,
}

#[derive(Resource, Default)]
pub struct Production {
    timers: HashMap<String, f32>,
}

#[derive(Resource, Default)]
pub struct PadSig(String);

#[derive(Resource, Default)]
pub struct Cooldowns {
    buy: f32,
    claim: f32,
}

const BELT_TOP: f32 = 0.85;
const BELT_SPEED: f32 = 6.0;
const GLOW_GREEN: u32 = 0x39D353;
const GLOW_RED: u32 = 0xD04040;

fn solid(b: &mut Build, parent: Entity, center: Vec3, he: Vec3) {
    let e = b.commands.spawn((Transform::from_translation(center), Visibility::default(), Collider::cuboid(he.x, he.y, he.z))).id();
    b.commands.entity(parent).add_child(e);
}

fn wall(b: &mut Build, parent: Entity, center: Vec3, size: Vec3) {
    b.boxp(parent, pal::CONCRETE, center + Vec3::Y * size.y / 2.0, size);
    b.boxp(parent, pal::CONCRETE_DARK, center + Vec3::Y * (size.y + 0.1), Vec3::new(size.x + 0.3, 0.2, size.z + 0.3));
    solid(b, parent, center + Vec3::Y * size.y / 2.0, size / 2.0);
    let long_x = size.x > size.z;
    let len = size.x.max(size.z);
    let n = (len / 9.0) as i32;
    for i in 0..n {
        let off = -len / 2.0 + (i as f32 + 0.5) * len / n as f32;
        let p = if long_x { Vec3::new(off, size.y + 0.45, 0.0) } else { Vec3::new(0.0, size.y + 0.45, off) };
        let g = b.glow(pal::AMBER);
        b.boxm(parent, g, center + p, Vec3::new(0.45, 0.45, 0.45));
    }
}

fn build_item(b: &mut Build, root: Entity, data: &GameData, item: &ItemDef, me_user: u64) {
    let v = Vec3::new;
    let p = item.pos();
    match &item.kind {
        ItemKind::Line { line } => {
            let l = &data.catalog.lines[*line];
            let (sx, z) = l.start;
            let cx = l.collector.0;
            let len = cx - sx;
            b.boxp(root, pal::CONCRETE_DARK, v((sx + cx) / 2.0, BELT_TOP / 2.0 - 0.05, z), v(len, BELT_TOP - 0.1, 2.2));
            b.boxp(root, pal::BLACK, v((sx + cx) / 2.0, BELT_TOP - 0.02, z), v(len, 0.05, 1.7));
            for s in [-1.0, 1.0] {
                b.boxp(root, pal::HAZARD, v((sx + cx) / 2.0, BELT_TOP - 0.02, z + 1.0 * s * 1.0 + 0.0), v(len, 0.07, 0.12));
            }
            let mut x = sx + 1.0;
            while x < cx - 1.0 {
                let g = b.glow(pal::AMBER);
                b.boxm(root, g, v(x, BELT_TOP + 0.02, z), v(0.5, 0.03, 0.3));
                x += 3.0;
            }
            b.boxp(root, pal::OLIVE_DARK, v(cx + 1.7, 1.1, z), v(3.0, 2.2, 3.4));
            b.boxp(root, pal::CONCRETE_DARK, v(cx + 0.1, 1.5, z), v(0.3, 2.0, 3.6));
            let g = b.glow(pal::AMBER);
            b.boxm(root, g, v(cx + 1.7, 2.25, z), v(2.0, 0.1, 2.6));
            solid(b, root, v(cx + 1.7, 1.1, z), v(1.5, 1.1, 1.7));
            solid(b, root, v((sx + cx) / 2.0, BELT_TOP / 2.0 - 0.05, z), v(len / 2.0, (BELT_TOP - 0.1) / 2.0, 1.1));
            solid(b, root, v(cx + 0.1, 1.5, z), v(0.15, 1.0, 1.8));
        }
        ItemKind::Dropper { line, .. } => {
            let z = data.catalog.lines[*line].start.1;
            for (dx, dz) in [(-1.1, -1.1), (1.1, -1.1), (-1.1, 1.1), (1.1, 1.1)] {
                b.boxp(root, pal::HAZARD, v(p.x + dx, 2.6, z + dz), v(0.3, 5.2, 0.3));
            }
            b.boxp(root, pal::BLACK, v(p.x, 5.7, z), v(2.8, 1.4, 2.8));
            b.boxp(root, pal::HAZARD, v(p.x, 4.9, z), v(1.6, 0.9, 1.6));
            b.boxp(root, pal::BLACK, v(p.x, 3.4, z), v(0.5, 2.2, 0.5));
            let g = b.glow(pal::AMBER);
            b.boxm(root, g, v(p.x, 6.45, z), v(1.8, 0.1, 1.8));
            for (dx, dz) in [(-1.1, -1.1), (1.1, -1.1), (-1.1, 1.1), (1.1, 1.1)] {
                solid(b, root, v(p.x + dx, 2.6, z + dz), v(0.15, 2.6, 0.15));
            }
            solid(b, root, v(p.x, 5.7, z), v(1.4, 0.7, 1.4));
            solid(b, root, v(p.x, 3.4, z), v(0.25, 1.1, 0.25));
        }
        ItemKind::Upgrader { line, .. } => {
            let z = data.catalog.lines[*line].start.1;
            for s in [-1.0, 1.0] {
                b.boxp(root, pal::CONCRETE, v(p.x, 2.0, z + 1.8 * s), v(1.4, 4.0, 0.6));
            }
            b.boxp(root, pal::CONCRETE, v(p.x, 4.2, z), v(1.4, 0.7, 4.2));
            let g = b.glow(pal::AMBER);
            b.boxm(root, g, v(p.x, 3.75, z), v(1.0, 0.15, 3.0));
            let g = b.glow(pal::AMBER);
            b.boxm(root, g, v(p.x, 1.0, z - 1.5), v(0.2, 1.2, 0.1));
            for s in [-1.0, 1.0] {
                solid(b, root, v(p.x, 2.0, z + 1.8 * s), v(0.7, 2.0, 0.3));
            }
            solid(b, root, v(p.x, 4.2, z), v(0.7, 0.35, 2.1));
        }
        ItemKind::Wall { size } => wall(b, root, v(p.x, 0.0, p.z), v(size.0, size.1, size.2)),
        ItemKind::Walls { parts } => {
            for (off, size) in parts {
                wall(b, root, p + v(off.0, off.1, off.2), v(size.0, size.1, size.2));
            }
        }
        ItemKind::Tower => {
            b.boxp(root, pal::CONCRETE, v(p.x, 5.0, p.z), v(4.0, 10.0, 4.0));
            b.boxp(root, pal::CONCRETE_DARK, v(p.x, 10.25, p.z), v(5.6, 0.5, 5.6));
            for (dx, dz) in [(-2.6, -2.6), (2.6, -2.6), (-2.6, 2.6), (2.6, 2.6)] {
                let g = b.glow(pal::AMBER);
                b.boxm(root, g, v(p.x + dx, 10.9, p.z + dz), v(0.35, 0.5, 0.35));
                b.boxp(root, pal::HAZARD, v(p.x + dx, 10.9, p.z + dz), v(0.15, 0.5, 0.15));
            }
            solid(b, root, v(p.x, 5.0, p.z), v(2.0, 5.0, 2.0));
            solid(b, root, v(p.x, 10.25, p.z), v(2.8, 0.25, 2.8));
            // Access ramp up the inner face (30 degrees, within the walkable slope limit).
            let dir = if p.x < 0.0 { 1.0 } else { -1.0 };
            let rot = Quat::from_rotation_z(-dir * 30f32.to_radians());
            let centre = v(p.x + dir * (2.8 + 8.66), 5.0, p.z);
            b.boxr(root, pal::CONCRETE_DARK, centre, v(20.0, 0.4, 3.2), rot);
            let e = b.commands.spawn((Transform::from_translation(centre).with_rotation(rot), Visibility::default(), Collider::cuboid(10.0, 0.2, 1.6))).id();
            b.commands.entity(root).add_child(e);
        }
        ItemKind::Turret { damage, range, rate } => {
            let head = b
                .commands
                .spawn((
                    Transform::from_xyz(p.x, p.y + 1.0, p.z),
                    Visibility::default(),
                    BaseTurret { user: me_user, plot_yaw: 0.0, damage: *damage, range: *range, rate: *rate, cooldown: 0.0 },
                ))
                .id();
            b.commands.entity(root).add_child(head);
            b.commands.entity(head).insert(Collider::cuboid(0.55, 0.4, 0.55));
            b.boxp(head, pal::GUNMETAL, Vec3::ZERO, v(1.1, 0.8, 1.1));
            b.boxp(head, pal::OLIVE_DARK, v(0.0, 0.5, 0.0), v(0.8, 0.3, 0.8));
            b.boxp(head, pal::BLACK, v(0.0, 0.0, 1.1), v(0.2, 0.2, 1.6));
            let g = b.glow(pal::AMBER);
            b.boxm(head, g, v(0.0, 0.2, 0.56), v(0.5, 0.08, 0.05));
        }
        ItemKind::WeaponRack { weapon } => {
            b.boxp(root, pal::WOOD, v(p.x, 1.1, p.z), v(0.4, 2.2, 3.6));
            b.boxp(root, pal::WOOD, v(p.x + 0.4, 0.5, p.z), v(0.5, 0.15, 3.4));
            let n = match weapon.as_str() {
                "rifle" => 3,
                "smg" => 4,
                "shotgun" => 3,
                "sniper" => 2,
                _ => 1,
            };
            for i in 0..n {
                let z = p.z - 1.3 + 2.6 * i as f32 / (n.max(2) - 1) as f32;
                b.boxp(root, pal::GUNMETAL, v(p.x + 0.45, 1.5, z), v(0.15, 0.18, 1.4));
                b.boxp(root, pal::OLIVE_DARK, v(p.x + 0.45, 1.5, z - 0.5), v(0.18, 0.25, 0.4));
            }
            let g = b.glow(pal::AMBER);
            b.boxm(root, g, v(p.x + 0.25, 2.35, p.z), v(0.08, 0.2, 1.2));
            solid(b, root, v(p.x, 1.1, p.z), v(0.5, 1.1, 1.8));
        }
        ItemKind::Barracks { .. } => {
            b.boxp(root, pal::OLIVE, v(p.x, 2.0, p.z), v(9.0, 4.0, 7.0));
            b.boxp(root, pal::OLIVE_DARK, v(p.x, 4.2, p.z), v(9.8, 0.4, 7.8));
            b.boxp(root, pal::BLACK, v(p.x - 4.52, 1.3, p.z), v(0.1, 2.6, 1.8));
            b.boxp(root, pal::KHAKI, v(p.x - 4.52, 3.0, p.z + 2.5), v(0.1, 1.0, 1.6));
            let g = b.glow(pal::AMBER);
            b.boxm(root, g, v(p.x - 4.55, 3.4, p.z), v(0.1, 0.2, 2.0));
            solid(b, root, v(p.x, 2.0, p.z), v(4.5, 2.0, 3.5));
            solid(b, root, v(p.x, 4.2, p.z), v(4.9, 0.2, 3.9));
        }
        ItemKind::MedStation => {
            b.boxp(root, pal::CONCRETE, v(p.x, 0.1, p.z), v(6.0, 0.2, 6.0));
            for (dx, dz) in [(-2.8, -2.8), (2.8, -2.8), (-2.8, 2.8), (2.8, 2.8)] {
                b.boxp(root, pal::KHAKI, v(p.x + dx, 1.5, p.z + dz), v(0.25, 3.0, 0.25));
            }
            b.boxp(root, pal::KHAKI, v(p.x, 3.1, p.z), v(6.4, 0.25, 6.4));
            let g = b.glow(GLOW_GREEN);
            b.boxm(root, g, v(p.x, 3.3, p.z), v(2.4, 0.1, 0.7));
            let g = b.glow(GLOW_GREEN);
            b.boxm(root, g, v(p.x, 3.3, p.z), v(0.7, 0.1, 2.4));
            let g = b.glow(GLOW_GREEN);
            b.boxm(root, g, v(p.x, 0.25, p.z), v(4.0, 0.06, 4.0));
            for (dx, dz) in [(-2.8, -2.8), (2.8, -2.8), (-2.8, 2.8), (2.8, 2.8)] {
                solid(b, root, v(p.x + dx, 1.5, p.z + dz), v(0.125, 1.5, 0.125));
            }
            solid(b, root, v(p.x, 3.1, p.z), v(3.2, 0.125, 3.2));
        }
        ItemKind::ArmorDepot { .. } => {
            b.boxp(root, pal::CONCRETE_DARK, v(p.x, 1.75, p.z), v(7.0, 3.5, 5.0));
            b.boxp(root, pal::CONCRETE, v(p.x, 3.6, p.z), v(7.4, 0.3, 5.4));
            let g = b.glow(0x4DA6FF);
            b.boxm(root, g, v(p.x - 3.55, 1.9, p.z), v(0.1, 0.4, 3.6));
            b.boxp(root, pal::BLACK, v(p.x - 3.52, 1.0, p.z), v(0.1, 2.0, 1.6));
            solid(b, root, v(p.x, 1.75, p.z), v(3.5, 1.75, 2.5));
            solid(b, root, v(p.x, 3.6, p.z), v(3.7, 0.15, 2.7));
        }
        ItemKind::VehiclePad { vehicle } => {
            let (w, l) = match vehicle.as_str() {
                "jeep" => (6.0, 9.0),
                "tank" => (8.0, 12.0),
                "heli" => (14.0, 14.0),
                _ => (10.0, 16.0),
            };
            b.boxp(root, pal::CONCRETE_DARK, v(p.x, 0.07, p.z), v(w, 0.14, l));
            for (sx, sz) in [(1.0, 0.0), (-1.0, 0.0)] {
                b.boxp(root, pal::HAZARD, v(p.x + sx * (w / 2.0 - 0.2), 0.16, p.z), v(0.3, 0.04, l));
                let _ = sz;
            }
            let g = b.glow(pal::AMBER);
            if vehicle == "heli" {
                b.boxm(root, g, v(p.x, 0.17, p.z), v(0.5, 0.03, 5.0));
                let g = b.glow(pal::AMBER);
                b.boxm(root, g, v(p.x - 2.5, 0.17, p.z), v(0.5, 0.03, 5.0));
                let g = b.glow(pal::AMBER);
                b.boxm(root, g, v(p.x + 2.5, 0.17, p.z), v(0.5, 0.03, 5.0));
                let g = b.glow(pal::AMBER);
                b.boxm(root, g, v(p.x, 0.17, p.z), v(5.0, 0.03, 0.5));
            } else {
                b.boxm(root, g, v(p.x, 0.17, p.z - l / 2.0 + 1.0), v(w - 1.5, 0.03, 0.4));
            }
        }
        ItemKind::Headquarters => {
            b.boxp(root, pal::CONCRETE, v(p.x, 2.5, p.z), v(12.0, 5.0, 8.0));
            b.boxp(root, pal::CONCRETE_DARK, v(p.x, 5.2, p.z), v(12.8, 0.4, 8.8));
            b.boxp(root, pal::BLACK, v(p.x, 8.0, p.z + 2.0), v(0.3, 5.6, 0.3));
            let g = b.glow(pal::AMBER);
            b.boxm(root, g, v(p.x, 11.0, p.z + 2.0), v(0.7, 0.7, 0.7));
            let g = b.glow(pal::AMBER);
            b.boxm(root, g, v(p.x, 2.2, p.z - 4.05), v(8.0, 0.4, 0.1));
            b.boxp(root, pal::BLACK, v(p.x, 1.4, p.z - 4.05), v(2.4, 2.8, 0.1));
            b.boxp(root, pal::OLIVE_DARK, v(p.x, 0.6, p.z - 5.2), v(2.2, 1.2, 0.8));
            let g = b.glow(pal::AMBER);
            b.boxm(root, g, v(p.x, 1.3, p.z - 5.2), v(2.0, 0.05, 0.6));
            solid(b, root, v(p.x, 2.5, p.z), v(6.0, 2.5, 4.0));
            solid(b, root, v(p.x, 5.2, p.z), v(6.4, 0.2, 4.4));
            solid(b, root, v(p.x, 8.0, p.z + 2.0), v(0.15, 2.8, 0.15));
            solid(b, root, v(p.x, 0.6, p.z - 5.2), v(1.1, 0.6, 0.4));
        }
    }
}

fn build_content(b: &mut Build, content: Entity, data: &GameData, owned: &[String], team_color: u32, user: u64) {
    let v = Vec3::new;
    // Always present: claim pad and a team banner at the gate.
    let (cx, cz) = data.catalog.claim_pad;
    b.boxp(content, pal::CONCRETE_DARK, v(cx, 0.1, cz), v(3.6, 0.2, 3.6));
    let g = b.glow(GLOW_GREEN);
    b.boxm(content, g, v(cx, 0.22, cz), v(3.0, 0.05, 3.0));
    b.boxp(content, pal::BLACK, v(-9.0, 3.0, 31.5), v(0.18, 6.0, 0.18));
    b.boxp(content, team_color, v(-9.0 + 0.1, 5.2, 31.5), v(0.06, 1.3, 2.0));
    b.boxp(content, pal::HAZARD, v(-9.0, 6.1, 31.5), v(0.3, 0.3, 0.3));
    solid(b, content, v(-9.0, 3.0, 31.5), v(0.09, 3.0, 0.09));
    for id in owned {
        if let Some(item) = data.catalog.get(id) {
            build_item(b, content, data, item, user);
        }
    }
}

fn plot_yaw(data: &GameData, plot: usize) -> f32 {
    data.map.plot(plot).1
}

/// Creates, updates and removes the base models of all players in the room.
pub fn sync_bases(
    mut b: Build,
    room: Res<Room>,
    econ: Res<Economy>,
    data: Res<GameData>,
    mut roots: Query<(Entity, &mut BaseRoot)>,
) {
    let mut wanted: HashMap<u64, (usize, u32, Vec<String>)> = HashMap::new();
    for (uid, p) in &room.players {
        let owned = if *uid == room.me {
            econ.base.owned.clone()
        } else {
            BaseState::from_json(&p.progress.as_ref().map(|x| x.base_json.clone()).unwrap_or_default()).owned
        };
        wanted.insert(*uid, (p.plot as usize, p.team, owned));
    }
    let mut seen = Vec::new();
    for (e, mut root) in &mut roots {
        let Some((plot, team, owned)) = wanted.get(&root.user) else {
            b.commands.entity(e).despawn();
            continue;
        };
        seen.push(root.user);
        if root.plot != *plot {
            b.commands.entity(e).despawn();
            seen.pop();
            continue;
        }
        let sig = format!("{team}|{}", owned.join(","));
        if sig != root.sig {
            b.commands.entity(root.content).despawn();
            let content = b.commands.spawn((Transform::default(), Visibility::default())).id();
            b.commands.entity(e).add_child(content);
            let color = room.settings.teams.get(*team as usize).map(|t| t.color).unwrap_or(pal::KHAKI);
            build_content(&mut b, content, &data, owned, color, root.user);
            root.content = content;
            root.sig = sig;
        }
    }
    for (uid, (plot, team, owned)) in wanted {
        if seen.contains(&uid) || plot >= data.map.plots.count {
            continue;
        }
        let (c, yaw) = data.map.plot(plot);
        let rot = Quat::from_rotation_y(yaw);
        let content = b.commands.spawn((Transform::default(), Visibility::default())).id();
        let color = room.settings.teams.get(team as usize).map(|t| t.color).unwrap_or(pal::KHAKI);
        build_content(&mut b, content, &data, &owned, color, uid);
        let root = b
            .commands
            .spawn((
                Transform::from_translation(c).with_rotation(rot),
                Visibility::default(),
                BaseRoot { user: uid, plot, sig: format!("{team}|{}", owned.join(",")), content },
                DespawnOnExit(AppState::InGame),
            ))
            .id();
        b.commands.entity(root).add_child(content);
    }
}

/// Spawns buy pads for everything currently purchasable in the local base.
#[allow(clippy::too_many_arguments)]
pub fn sync_pads(
    mut b: Build,
    room: Res<Room>,
    econ: Res<Economy>,
    data: Res<GameData>,
    mut sig: ResMut<PadSig>,
    roots: Query<(Entity, &BaseRoot)>,
    pads: Query<Entity, With<BuyPad>>,
) {
    let Some((root, _)) = roots.iter().find(|(_, r)| r.user == room.me) else { return };
    let new_sig = format!("{}|{}", root.index(), econ.base.owned.join(","));
    if new_sig == sig.0 {
        return;
    }
    sig.0 = new_sig;
    for e in &pads {
        b.commands.entity(e).despawn();
    }
    let ids: Vec<(String, Vec3)> =
        data.catalog.available(&econ.base).map(|i| (i.id.clone(), Vec3::new(i.button.0, 0.0, i.button.1))).collect();
    for (id, local) in ids {
        let pad = b
            .commands
            .spawn((Transform::from_translation(local), Visibility::default(), BuyPad { item: id, local }))
            .id();
        b.commands.entity(root).add_child(pad);
        b.boxp(pad, pal::CONCRETE_DARK, Vec3::new(0.0, 0.08, 0.0), Vec3::new(3.4, 0.16, 3.4));
        let g = b.glow(pal::AMBER);
        let inner = b.boxm(pad, g, Vec3::new(0.0, 0.18, 0.0), Vec3::new(2.8, 0.06, 2.8));
        b.commands.entity(inner).insert(PadFace);
    }
}

#[derive(Component)]
pub struct PadFace;

/// Colours pad faces by affordability: green buyable, amber short on cash, red locked.
pub fn pad_colors(
    mut b: Build,
    econ: Res<Economy>,
    data: Res<GameData>,
    pads: Query<(&BuyPad, &Children)>,
    mut faces: Query<&mut MeshMaterial3d<StandardMaterial>, With<PadFace>>,
) {
    let (green, amber, red) = (b.glow(GLOW_GREEN), b.glow(pal::AMBER), b.glow(GLOW_RED));
    for (pad, children) in &pads {
        let Some(item) = data.catalog.get(&pad.item) else { continue };
        let want = if econ.rank < item.rank {
            &red
        } else if econ.cash < item.cost {
            &amber
        } else {
            &green
        };
        for c in children.iter() {
            if let Ok(mut m) = faces.get_mut(c) {
                if m.0 != *want {
                    m.0 = want.clone();
                }
            }
        }
    }
}

fn local_pos(data: &GameData, plot: usize, world: Vec3) -> Vec3 {
    let (c, yaw) = data.map.plot(plot);
    Quat::from_rotation_y(-yaw) * (world - c)
}

/// Standing on a buy pad purchases the item.
#[allow(clippy::too_many_arguments)]
pub fn buy_on_pads(
    time: Res<Time>,
    room: Res<Room>,
    data: Res<GameData>,
    vitals: Res<Vitals>,
    seat: Res<Seat>,
    mut econ: ResMut<Economy>,
    mut feed: ResMut<Feed>,
    mut cd: ResMut<Cooldowns>,
    pads: Query<&BuyPad>,
    me: Query<&Transform, With<LocalPlayer>>,
) {
    cd.buy -= time.delta_secs();
    let Ok(pt) = me.single() else { return };
    if cd.buy > 0.0 || !vitals.alive() || seat.0.is_some() {
        return;
    }
    let l = local_pos(&data, room.my_plot, pt.translation);
    for pad in &pads {
        if (l.x - pad.local.x).abs() > 1.6 || (l.z - pad.local.z).abs() > 1.6 || l.y > 2.5 {
            continue;
        }
        let rank = econ.rank;
        let cash = econ.cash;
        match data.catalog.buy(&pad.item, &mut econ.base.clone(), cash, rank) {
            Ok(rest) => {
                let cost = cash - rest;
                let name = data.catalog.get(&pad.item).map(|i| i.name.clone()).unwrap_or_default();
                econ.base.owned.push(pad.item.clone());
                econ.cash = rest;
                let now = time.elapsed_secs_f64();
                if let Some(rank) = econ.add_xp(purchase_xp(cost)) {
                    feed.toast(now, format!("Promoted to {rank}!"));
                }
                econ.save_now();
                feed.toast(now, format!("Built {name}"));
                cd.buy = 0.5;
                return;
            }
            Err(e) => {
                if cd.buy < -1.5 {
                    let name = data.catalog.get(&pad.item).map(|i| i.name.clone()).unwrap_or_default();
                    feed.toast(time.elapsed_secs_f64(), format!("{name}: {e}"));
                    cd.buy = 0.0;
                }
            }
        }
    }
}

/// Drops ore from droppers, moves it down the belt, upgrades and banks it.
#[allow(clippy::too_many_arguments)]
pub fn production(
    time: Res<Time>,
    mut b: Build,
    room: Res<Room>,
    data: Res<GameData>,
    mut econ: ResMut<Economy>,
    mut prod: ResMut<Production>,
    roots: Query<(Entity, &BaseRoot)>,
    mut ores: Query<(Entity, &mut Ore, &mut Transform)>,
) {
    let Some((root, _)) = roots.iter().find(|(_, r)| r.user == room.me) else { return };
    let dt = time.delta_secs().min(0.1);
    let cat = &data.catalog;

    let mut spawns = Vec::new();
    for item in cat.owns_kind(&econ.base) {
        if let ItemKind::Dropper { line, value, interval } = item.kind {
            if !cat.has_line(&econ.base, line) {
                continue;
            }
            let t = prod.timers.entry(item.id.clone()).or_insert(0.0);
            *t += dt;
            if *t >= interval {
                *t -= interval;
                spawns.push((line, value, item.pos().x));
            }
        }
    }
    for (line, value, x) in spawns {
        let z = cat.lines[line].start.1;
        let mat = b.mat(pal::ORE);
        let mesh = b.art.cube.clone();
        let e = b
            .commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(mat),
                Transform::from_xyz(x, 3.4, z).with_scale(Vec3::splat(0.5)),
                Ore { value, line, vy: 0.0, upgrades: 0 },
            ))
            .id();
        b.commands.entity(root).add_child(e);
    }

    let mult = rebirth_multiplier(econ.rebirths) * room.settings.cash_multiplier;
    let upgraders: Vec<(usize, f32, f32)> = cat
        .owns_kind(&econ.base)
        .filter_map(|i| match i.kind {
            ItemKind::Upgrader { line, mult } => Some((line, i.pos().x, mult)),
            _ => None,
        })
        .collect();
    let mut banked = 0.0;
    for (e, mut ore, mut t) in &mut ores {
        let end = cat.lines[ore.line].collector.0 - 0.8;
        if t.translation.y > BELT_TOP + 0.25 {
            ore.vy -= 22.0 * dt;
            t.translation.y = (t.translation.y + ore.vy * dt).max(BELT_TOP + 0.25);
        } else {
            let from = t.translation.x;
            t.translation.x += BELT_SPEED * dt;
            for (line, ux, m) in &upgraders {
                if *line == ore.line && from < *ux && t.translation.x >= *ux {
                    ore.value *= m;
                    ore.upgrades += 1;
                    let glow = b.glow(pal::AMBER);
                    b.commands.entity(e).insert(MeshMaterial3d(glow));
                }
            }
            if t.translation.x >= end {
                banked += ore.value as f64 * mult as f64;
                b.commands.entity(e).despawn();
            }
        }
    }
    if banked > 0.0 {
        econ.bank += banked;
    }
}

/// Standing on the claim pad moves banked income into cash.
pub fn claim_pad(
    time: Res<Time>,
    room: Res<Room>,
    data: Res<GameData>,
    mut econ: ResMut<Economy>,
    mut feed: ResMut<Feed>,
    mut cd: ResMut<Cooldowns>,
    me: Query<&Transform, With<LocalPlayer>>,
) {
    cd.claim -= time.delta_secs();
    let Ok(pt) = me.single() else { return };
    let l = local_pos(&data, room.my_plot, pt.translation);
    let (cx, cz) = data.catalog.claim_pad;
    if (l.x - cx).abs() > 1.8 || (l.z - cz).abs() > 1.8 || l.y > 2.5 || econ.bank < 1.0 {
        return;
    }
    let amount = econ.bank.floor();
    econ.bank -= amount;
    econ.cash += amount as i64;
    econ.mark_dirty();
    if cd.claim <= 0.0 {
        feed.toast(time.elapsed_secs_f64(), format!("+${amount}"));
        cd.claim = 1.5;
    }
}

/// Offers rebirth at the headquarters and vehicle spawning at pads.
pub fn offer_actions(
    room: Res<Room>,
    data: Res<GameData>,
    econ: Res<Economy>,
    seat: Res<Seat>,
    vitals: Res<Vitals>,
    mut inter: ResMut<Interact>,
    me: Query<&Transform, With<LocalPlayer>>,
) {
    let Ok(pt) = me.single() else { return };
    if seat.0.is_some() || !vitals.alive() {
        return;
    }
    let l = local_pos(&data, room.my_plot, pt.translation);
    let (_, yaw) = data.map.plot(room.my_plot);
    for item in data.catalog.owns_kind(&econ.base) {
        let p = item.pos();
        let d = Vec3::new(l.x - p.x, 0.0, l.z - p.z).length();
        match &item.kind {
            ItemKind::Headquarters if d < 9.0 => {
                let price = rebirth_price(econ.rebirths);
                inter.offer(d, format!("Rebirth (${price}, +25% income)"), Action::Rebirth);
            }
            ItemKind::VehiclePad { vehicle } if d < 7.0 => {
                let pos = data.map.plot_to_world(room.my_plot, p);
                inter.offer(d, format!("Deploy {vehicle}"), Action::SpawnVehicle { def: vehicle.clone(), pos, yaw });
            }
            _ => {}
        }
    }
}

pub fn handle_requests(
    mut rebirth: MessageReader<RebirthRequest>,
    mut spawn: MessageReader<SpawnVehicleRequest>,
    time: Res<Time>,
    room: Res<Room>,
    data: Res<GameData>,
    net: Res<Net>,
    mut econ: ResMut<Economy>,
    mut feed: ResMut<Feed>,
    mut prod: ResMut<Production>,
    vehicles: Query<&NetObject, With<VehicleBody>>,
    ores: Query<Entity, With<Ore>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    for _ in rebirth.read() {
        if !can_rebirth(&econ.base, econ.cash, econ.rebirths) {
            feed.toast(now, format!("Rebirth needs ${}", rebirth_price(econ.rebirths)));
            continue;
        }
        econ.rebirths += 1;
        econ.cash = room.settings.starting_cash;
        econ.bank = 0.0;
        econ.base = BaseState::default();
        econ.save_now();
        prod.timers.clear();
        for e in &ores {
            commands.entity(e).despawn();
        }
        let r = econ.rebirths;
        feed.toast(now, format!("Reborn! Rebirth {r}: x{:.2} income", rebirth_multiplier(r)));
    }
    for req in spawn.read() {
        let Some(def) = data.vehicles.get(&req.def) else { continue };
        for o in &vehicles {
            if o.spawned_by == room.me && o.def == req.def {
                net.send(C::Despawn(pb::DespawnObject { id: o.id, destroyed: false }));
            }
        }
        net.send(C::Spawn(pb::SpawnObject {
            kind: ObjectKind::Vehicle as i32,
            def: req.def.clone(),
            state: Some(pb::ObjectState {
                pos: pv(req.pos + Vec3::Y * (def.half_extents[1] + 0.8)),
                rot: pq(Quat::from_rotation_y(req.yaw)),
                vel: pv(Vec3::ZERO),
                angvel: pv(Vec3::ZERO),
                health: def.health,
                extra: vec![0.0; 4],
            }),
            unowned: true,
        }));
        feed.toast(now, format!("Deployed {}", req.def));
    }
}

/// Auto turrets on the local base fire at visible enemies.
#[allow(clippy::too_many_arguments)]
pub fn turrets(
    time: Res<Time>,
    mut b: Build,
    room: Res<Room>,
    data: Res<GameData>,
    net: Res<Net>,
    mut shots: ResMut<OutShots>,
    rapier: ReadRapierContext,
    mut heads: Query<(&mut BaseTurret, &mut Transform, &GlobalTransform)>,
    targets: Query<(Entity, &NetObject, &GlobalTransform), Without<BaseTurret>>,
) {
    let Ok(ctx) = rapier.single() else { return };
    let dt = time.delta_secs();
    for (mut turret, mut tr, gt) in &mut heads {
        if turret.user != room.me {
            continue;
        }
        turret.cooldown = (turret.cooldown - dt).max(0.0);
        turret.plot_yaw = plot_yaw(&data, room.my_plot);
        let origin = gt.translation() + Vec3::Y * 0.2;
        let mut best: Option<(f32, Entity, u64, Vec3)> = None;
        for (e, obj, tgt) in &targets {
            let enemy = match obj.kind {
                ObjectKind::Player => {
                    obj.owner != room.me && obj.health > 0.0 && obj.extra.get(2).copied().unwrap_or(0.0) < 0.5 && room.is_enemy(obj.owner)
                }
                ObjectKind::Vehicle => obj.seats.first().is_some_and(|&s| s != 0 && room.is_enemy(s)),
                _ => false,
            };
            if !enemy {
                continue;
            }
            let pos = tgt.translation() + if obj.kind == ObjectKind::Player { Vec3::Y * 0.2 } else { Vec3::ZERO };
            let d = pos.distance(origin);
            if d <= turret.range && best.as_ref().is_none_or(|x| d < x.0) {
                // Line of sight: the first thing hit must be the target itself.
                let dir = (pos - origin).normalize_or_zero();
                if let Some((hit, _)) = ctx.cast_ray(origin, dir, d + 1.0, true, QueryFilter::default().exclude_sensors()) {
                    if hit == e {
                        best = Some((d, e, obj.id, pos));
                    }
                }
            }
        }
        let Some((_, _, id, pos)) = best else { continue };
        let local = Quat::from_rotation_y(-turret.plot_yaw) * (pos - origin);
        tr.rotation = Quat::from_rotation_y(local.x.atan2(local.z));
        if turret.cooldown <= 0.0 {
            turret.cooldown = 1.0 / turret.rate.max(0.1);
            let dir = (pos - origin).normalize_or_zero();
            spawn_tracer(&mut b, origin + dir * 1.5, pos);
            net.send(C::Hit(pb::HitReport { target_id: id, damage: turret.damage, weapon: "turret".into(), point: pv(pos) }));
            shots.0.push(pb::ProjectileFired { shooter: 0, weapon: "turret".into(), origin: pv(origin + dir * 1.5), dir: pv(dir), seed: 0 });
        }
    }
}

/// Med station heals and the armor depot restores armor, in the local base.
pub fn support_buildings(
    time: Res<Time>,
    room: Res<Room>,
    data: Res<GameData>,
    econ: Res<Economy>,
    mut vitals: ResMut<Vitals>,
    me: Query<&Transform, With<LocalPlayer>>,
) {
    let Ok(pt) = me.single() else { return };
    if !vitals.alive() {
        return;
    }
    let l = local_pos(&data, room.my_plot, pt.translation);
    let dt = time.delta_secs();
    for item in data.catalog.owns_kind(&econ.base) {
        let p = item.pos();
        let d = Vec3::new(l.x - p.x, 0.0, l.z - p.z).length();
        match item.kind {
            ItemKind::MedStation if d < 4.5 && l.y < 3.0 => {
                vitals.hp = (vitals.hp + 25.0 * dt).min(vitals.max_hp);
            }
            ItemKind::ArmorDepot { armor } if d < 5.0 && l.y < 3.0 => {
                vitals.max_armor = armor;
                vitals.armor = (vitals.armor + 20.0 * dt).min(armor);
            }
            _ => {}
        }
    }
}

pub fn reset_state(mut prod: ResMut<Production>, mut sig: ResMut<PadSig>, mut cd: ResMut<Cooldowns>) {
    prod.timers.clear();
    sig.0.clear();
    *cd = Cooldowns::default();
}

/// Income per second shown in the HUD (before the room multiplier).
pub fn income(data: &GameData, econ: &Economy, room: &Room) -> f32 {
    data.catalog.income_per_sec(&econ.base) * rebirth_multiplier(econ.rebirths) * room.settings.cash_multiplier
}
