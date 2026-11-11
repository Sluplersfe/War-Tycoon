//! The local character: movement, third-person camera, seats, health,
//! death and respawn, and the shared "press E" interaction.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};
use bevy_rapier3d::prelude::*;
use frontline_proto::pb::{self, ObjectKind, client_msg::Msg as C};
use frontline_sim::defs::VehicleClass;

use crate::art::Spinner;
use crate::net::Net;
use crate::objects::{MotionVel, NetObject, VehicleBody, state_of};
use crate::state::{Economy, Feed, GameData, Room};
use crate::world::MainCamera;

pub const WALK: f32 = 7.0;
pub const SPRINT: f32 = 11.0;
pub const JUMP: f32 = 7.5;
pub const GRAVITY: f32 = 20.0;
pub const RESPAWN_SECS: f64 = 5.0;

#[derive(Component)]
pub struct LocalPlayer;

#[derive(Resource, Default)]
pub struct Look {
    pub yaw: f32,
    pub pitch: f32,
    /// Field-of-view divisor while aiming (1 = none).
    pub zoom: f32,
    /// 0..1 aim-down-sights progress.
    pub ads: f32,
    /// Fully aimed through a magnified scope (draw the scope overlay).
    pub scoped: bool,
    /// Debug chase-camera distance on foot (0 = first person).
    pub chase: f32,
}

#[derive(Resource)]
pub struct Vitals {
    pub hp: f32,
    pub max_hp: f32,
    pub armor: f32,
    pub max_armor: f32,
    pub last_damage: f64,
    pub dead_until: Option<f64>,
    pub protect_until: f64,
    pub last_attacker: u64,
    pub last_weapon: String,
    pub vy: f32,
}

impl Default for Vitals {
    fn default() -> Self {
        Self {
            hp: 100.0,
            max_hp: 100.0,
            armor: 0.0,
            max_armor: 0.0,
            last_damage: -100.0,
            dead_until: None,
            protect_until: 0.0,
            last_attacker: 0,
            last_weapon: String::new(),
            vy: 0.0,
        }
    }
}

impl Vitals {
    pub fn alive(&self) -> bool {
        self.dead_until.is_none()
    }

    /// Applies incoming damage, armor first. Returns true if it was applied.
    pub fn damage(&mut self, now: f64, amount: f32, from: u64, weapon: &str) -> bool {
        if !self.alive() || now < self.protect_until {
            return false;
        }
        let absorbed = amount.min(self.armor) * 0.7;
        self.armor = (self.armor - amount).max(0.0);
        self.hp -= amount - absorbed;
        self.last_damage = now;
        if from != 0 {
            self.last_attacker = from;
            self.last_weapon = weapon.to_string();
        }
        true
    }
}

#[derive(Clone, Copy)]
pub struct SeatInfo {
    pub vehicle: Entity,
    pub id: u64,
    pub index: usize,
    pub class: VehicleClass,
    pub last_health: f32,
}

#[derive(Resource, Default)]
pub struct Seat(pub Option<SeatInfo>);

impl Seat {
    pub fn driving(&self) -> Option<SeatInfo> {
        self.0.filter(|s| s.index == 0)
    }
}

/// Whether the game (not the UI) has the mouse.
#[derive(Resource, Default)]
pub struct Controls {
    pub captured: bool,
    pub ui_wants_cursor: bool,
}

/// The best "press E" action this frame.
#[derive(Clone)]
pub enum Action {
    Enter { id: u64, seat: u32 },
    SpawnVehicle { def: String, pos: Vec3, yaw: f32 },
    Rebirth,
}

#[derive(Resource, Default)]
pub struct Interact {
    pub best: Option<(f32, String, Action)>,
}

impl Interact {
    pub fn offer(&mut self, dist: f32, label: impl Into<String>, action: Action) {
        if self.best.as_ref().is_none_or(|(d, _, _)| dist < *d) {
            self.best = Some((dist, label.into(), action));
        }
    }
}

pub fn seat_offset(class: VehicleClass, index: usize) -> Vec3 {
    let v = Vec3::new;
    let table: &[Vec3] = match class {
        VehicleClass::Car => &[v(-0.45, 0.55, 0.2), v(0.45, 0.55, 0.2), v(-0.4, 0.55, -1.1), v(0.4, 0.55, -1.1)],
        VehicleClass::Tank => &[v(0.0, 1.0, 0.0), v(0.0, 1.6, -0.6)],
        VehicleClass::Heli => &[v(-0.5, 0.2, 3.0), v(0.5, 0.2, 3.0), v(-0.6, 0.2, 0.0), v(0.6, 0.2, 0.0)],
        VehicleClass::Plane => &[v(0.0, 0.6, 0.3)],
        VehicleClass::Prop => &[v(0.0, 1.0, 0.0)],
    };
    table.get(index).copied().unwrap_or(v(0.0, 1.0, 0.0))
}

/// Completes the local character once its object exists.
pub fn attach_local_player(
    mut commands: Commands,
    room: Res<Room>,
    mut look: ResMut<Look>,
    mut vitals: ResMut<Vitals>,
    q: Query<(Entity, &NetObject, &Transform), Without<LocalPlayer>>,
) {
    for (e, obj, t) in &q {
        if obj.kind == ObjectKind::Player && obj.owner == room.me {
            commands.entity(e).insert((
                LocalPlayer,
                Collider::capsule_y(0.5, 0.4),
                KinematicCharacterController {
                    offset: CharacterLength::Absolute(0.02),
                    max_slope_climb_angle: 50f32.to_radians(),
                    min_slope_slide_angle: 35f32.to_radians(),
                    autostep: Some(CharacterAutostep {
                        max_height: CharacterLength::Absolute(0.45),
                        min_width: CharacterLength::Absolute(0.2),
                        include_dynamic_bodies: false,
                    }),
                    snap_to_ground: Some(CharacterLength::Absolute(0.3)),
                    apply_impulse_to_dynamic_bodies: true,
                    ..default()
                },
            ));
            let (yaw, _, _) = t.rotation.to_euler(EulerRot::YXZ);
            look.yaw = yaw;
            look.pitch = -0.1;
            *vitals = Vitals { protect_until: 0.0, ..default() };
        }
    }
}

pub fn grab_cursor(
    mut controls: ResMut<Controls>,
    mut cursor: Single<&mut CursorOptions>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    mut forced: Local<Option<bool>>,
) {
    let force = *forced.get_or_insert_with(|| std::env::var("FF_AUTO").is_ok());
    let focused = force || windows.iter().any(|w| w.focused);
    if controls.ui_wants_cursor || !focused {
        controls.captured = false;
    } else if mouse.just_pressed(MouseButton::Left) || !controls.captured {
        controls.captured = true;
    }
    let (vis, grab) = if controls.captured { (false, CursorGrabMode::Locked) } else { (true, CursorGrabMode::None) };
    if cursor.visible != vis {
        cursor.visible = vis;
    }
    if cursor.grab_mode != grab {
        cursor.grab_mode = grab;
    }
}

pub fn release_cursor(mut cursor: Single<&mut CursorOptions>, mut controls: ResMut<Controls>) {
    cursor.visible = true;
    cursor.grab_mode = CursorGrabMode::None;
    controls.captured = false;
}

pub fn mouse_look(
    motion: Res<AccumulatedMouseMotion>,
    controls: Res<Controls>,
    mut look: ResMut<Look>,
    settings: Res<crate::settings::Settings>,
) {
    if !controls.captured {
        return;
    }
    let sens = 0.0025 * settings.sensitivity / look.zoom.max(1.0);
    let flip = if settings.invert_y { -1.0 } else { 1.0 };
    look.yaw -= motion.delta.x * sens;
    look.pitch = (look.pitch - motion.delta.y * sens * flip).clamp(-1.4, 1.3);
}

#[allow(clippy::too_many_arguments)]
pub fn move_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    controls: Res<Controls>,
    look: Res<Look>,
    seat: Res<Seat>,
    mut vitals: ResMut<Vitals>,
    data: Res<GameData>,
    loadout: Res<crate::combat::Loadout>,
    armory: Res<crate::guns::Armory>,
    mut q: Query<
        (
            &mut Transform,
            &mut KinematicCharacterController,
            Option<&KinematicCharacterControllerOutput>,
            &mut MotionVel,
            &mut NetObject,
        ),
        With<LocalPlayer>,
    >,
) {
    let Ok((mut t, mut kcc, out, mut mv, mut obj)) = q.single_mut() else { return };
    let dt = time.delta_secs().min(0.05);
    obj.health = vitals.hp.max(0.0);
    obj.extra = vec![
        look.pitch,
        data.weapon_index(loadout.current_id()) as f32,
        if seat.0.is_some() { 1.0 } else { 0.0 },
        armory.get(loadout.current_id()).pack() as f32,
    ];
    if seat.0.is_some() || !vitals.alive() {
        kcc.translation = None;
        mv.0 = Vec3::ZERO;
        vitals.vy = 0.0;
        return;
    }
    let mut wish = Vec3::ZERO;
    if controls.captured {
        if keys.pressed(KeyCode::KeyW) {
            wish.z += 1.0;
        }
        if keys.pressed(KeyCode::KeyS) {
            wish.z -= 1.0;
        }
        if keys.pressed(KeyCode::KeyA) {
            wish.x += 1.0;
        }
        if keys.pressed(KeyCode::KeyD) {
            wish.x -= 1.0;
        }
    }
    let speed = if keys.pressed(KeyCode::ShiftLeft) && wish.z > 0.0 { SPRINT } else { WALK };
    let yaw = Quat::from_rotation_y(look.yaw);
    let horiz = yaw * wish.normalize_or_zero() * speed;
    let grounded = out.map(|o| o.grounded).unwrap_or(false);
    if grounded {
        vitals.vy = -1.0;
        if controls.captured && keys.just_pressed(KeyCode::Space) {
            vitals.vy = JUMP;
        }
    } else {
        vitals.vy -= GRAVITY * dt;
    }
    let vel = Vec3::new(horiz.x, vitals.vy, horiz.z);
    kcc.translation = Some(vel * dt);
    mv.0 = vel;
    t.rotation = yaw;
}

/// Finds which vehicle seat (if any) the server says we occupy.
#[allow(clippy::too_many_arguments)]
pub fn track_seat(
    mut commands: Commands,
    room: Res<Room>,
    mut seat: ResMut<Seat>,
    mut vitals: ResMut<Vitals>,
    vehicles: Query<(Entity, &NetObject, &VehicleBody, &Transform), Without<LocalPlayer>>,
    mut me: Query<(Entity, &mut Transform), With<LocalPlayer>>,
    time: Res<Time>,
    mut feed: ResMut<Feed>,
) {
    let found = vehicles.iter().find_map(|(e, o, vb, _)| {
        o.seat_of(room.me).map(|i| SeatInfo { vehicle: e, id: o.id, index: i, class: vb.class, last_health: o.health })
    });
    let Ok((pe, mut pt)) = me.single_mut() else { return };
    match (seat.0, found) {
        (None, Some(s)) => {
            commands.entity(pe).insert(ColliderDisabled);
            seat.0 = Some(s);
        }
        (Some(old), None) => {
            commands.entity(pe).remove::<ColliderDisabled>();
            if let Ok((_, _, vb, vt)) = vehicles.get(old.vehicle) {
                let side = vt.rotation * Vec3::X;
                pt.translation = vt.translation + Vec3::new(side.x, 0.0, side.z).normalize_or_zero() * (vb.half.x + 1.2) + Vec3::Y * 1.0;
            } else {
                // Vehicle vanished under us: destroyed or removed.
                pt.translation += Vec3::Y * 1.5;
                if old.last_health <= 0.0 {
                    vitals.damage(time.elapsed_secs_f64(), 60.0, 0, "explosion");
                    feed.toast(time.elapsed_secs_f64(), "Your vehicle was destroyed");
                }
            }
            pt.rotation = Quat::IDENTITY;
            seat.0 = None;
        }
        (Some(_), Some(s)) => seat.0 = Some(s),
        (None, None) => {}
    }
}

/// Keeps a seated character on its seat (after physics, before transforms propagate).
pub fn follow_seat(
    seat: Res<Seat>,
    look: Res<Look>,
    vehicles: Query<&Transform, (With<VehicleBody>, Without<LocalPlayer>)>,
    mut me: Query<(&mut Transform, &Children), With<LocalPlayer>>,
    mut vis: Query<&mut Visibility>,
    vitals: Res<Vitals>,
) {
    let Ok((mut pt, children)) = me.single_mut() else { return };
    // On foot the camera is the eyes; the view model draws the weapon.
    let first_person = seat.0.is_none() && look.chase == 0.0;
    let mut hidden = !vitals.alive();
    if let Some(s) = seat.0 {
        if let Ok(vt) = vehicles.get(s.vehicle) {
            pt.translation = vt.translation + vt.rotation * seat_offset(s.class, s.index);
            pt.rotation = vt.rotation;
        }
        hidden |= s.class != VehicleClass::Car;
    }
    for c in children.iter() {
        if let Ok(mut v) = vis.get_mut(c) {
            let hide = hidden || first_person;
            let want = if hide { Visibility::Hidden } else { Visibility::Inherited };
            if *v != want {
                *v = want;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn camera_follow(
    look: Res<Look>,
    seat: Res<Seat>,
    me: Query<(Entity, &Transform), (With<LocalPlayer>, Without<MainCamera>)>,
    vehicles: Query<(&Transform, &VehicleBody), (Without<LocalPlayer>, Without<MainCamera>)>,
    mut cam: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
    rapier: ReadRapierContext,
    settings: Res<crate::settings::Settings>,
) {
    let Ok((pe, pt)) = me.single() else { return };
    let Ok((mut ct, mut proj)) = cam.single_mut() else { return };
    let rot = Quat::from_euler(EulerRot::YXZ, look.yaw, -look.pitch, 0.0);
    let dir = rot * Vec3::Z;
    let (pivot, dist, exclude) = match seat.0.and_then(|s| vehicles.get(s.vehicle).ok().map(|v| (s, v))) {
        Some((s, (vt, vb))) => {
            let d = match vb.class {
                VehicleClass::Car => 8.0,
                VehicleClass::Tank => 11.0,
                VehicleClass::Heli => 16.0,
                VehicleClass::Plane => 18.0,
                VehicleClass::Prop => 6.0,
            };
            (vt.translation + Vec3::Y * (vb.half.y + 1.5), d, Some(s.vehicle))
        }
        None => {
            if look.chase != 0.0 {
                (pt.translation + Vec3::Y * 0.9 + rot * Vec3::new(-0.6, 0.0, 0.0), look.chase.abs(), None)
            } else {
                (pt.translation + Vec3::Y * 0.62 + dir * 0.12, 0.0, None)
            }
        }
    };
    let mut want = pivot - dir * dist;
    if let Some(ctx) = rapier.single().ok().filter(|_| dist > 0.0) {
        let mut filter = QueryFilter::default().exclude_collider(pe).exclude_sensors();
        if let Some(v) = exclude {
            filter = filter.exclude_collider(v).exclude_rigid_body(v);
        }
        let back = want - pivot;
        if let Some((_, toi)) = ctx.cast_ray(pivot, back.normalize_or_zero(), back.length(), true, filter) {
            want = pivot + back.normalize_or_zero() * (toi - 0.3).max(0.3);
        }
    }
    ct.translation = want;
    ct.look_to(dir, Vec3::Y);
    if look.chase < 0.0 && seat.0.is_none() {
        // Debug front view: stand ahead of the soldier, looking back at it.
        let fwd = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::Z);
        let right = Vec3::Y.cross(fwd);
        ct.translation = pt.translation + fwd * -look.chase - right * 1.2 + Vec3::Y * 0.6;
        ct.look_at(pt.translation + Vec3::Y * 0.2 - right * 0.2, Vec3::Y);
    }
    if let Projection::Perspective(p) = proj.as_mut() {
        let base = settings.fov.to_radians();
        let fov = 2.0 * ((base / 2.0).tan() / look.zoom.max(1.0)).atan();
        if (p.fov - fov).abs() > 1e-3 {
            p.fov = fov;
        }
    }
}

/// Death, respawn, regen and the out-of-bounds kill plane.
#[allow(clippy::too_many_arguments)]
pub fn life_cycle(
    time: Res<Time>,
    mut vitals: ResMut<Vitals>,
    net: Res<Net>,
    room: Res<Room>,
    data: Res<GameData>,
    econ: Res<Economy>,
    mut seat: ResMut<Seat>,
    mut feed: ResMut<Feed>,
    mut loadout: ResMut<crate::combat::Loadout>,
    mut me: Query<&mut Transform, With<LocalPlayer>>,
    vehicles: Query<(&NetObject, &Transform, Option<&Velocity>), Without<LocalPlayer>>,
) {
    let now = time.elapsed_secs_f64();
    let Ok(mut t) = me.single_mut() else { return };

    // Base bonuses.
    let owns = |id: &str| econ.base.owns(id);
    vitals.max_hp = 100.0 + if owns("barracks") { 25.0 } else { 0.0 };
    vitals.max_armor = if owns("armor_depot") { 50.0 } else { 0.0 };

    if vitals.alive() {
        if t.translation.y < -3.0 && seat.0.is_none() {
            vitals.hp = 0.0;
            vitals.last_attacker = 0;
        }
        if vitals.hp <= 0.0 {
            let recent = now - vitals.last_damage < 10.0;
            let killer = if recent { vitals.last_attacker } else { 0 };
            let weapon = if recent { vitals.last_weapon.clone() } else { String::new() };
            net.send(C::Death(pb::DeathReport { killer_id: killer, weapon }));
            if let Some(s) = seat.0.take() {
                if let Ok((o, vt, vel)) = vehicles.get(s.vehicle) {
                    let state = (o.owner == room.me).then(|| state_of(o, vt, vel));
                    net.send(C::Release(pb::ReleaseObject { id: s.id, state }));
                }
            }
            vitals.dead_until = Some(now + RESPAWN_SECS);
            feed.toast(now, "You were eliminated");
        } else if now - vitals.last_damage > 6.0 && vitals.hp < vitals.max_hp {
            vitals.hp = (vitals.hp + 8.0 * time.delta_secs()).min(vitals.max_hp);
        }
    } else if vitals.dead_until.is_some_and(|d| now >= d) {
        vitals.dead_until = None;
        vitals.hp = vitals.max_hp;
        vitals.armor = vitals.max_armor;
        vitals.protect_until = now + 3.0;
        vitals.vy = 0.0;
        loadout.refill();
        let (_, yaw) = data.map.plot(room.my_plot);
        t.translation = data.map.plot_spawn(room.my_plot);
        t.rotation = Quat::from_rotation_y(yaw);
    }
}

/// Offers entering nearby vehicles and handles E presses.
#[allow(clippy::too_many_arguments)]
pub fn interact(
    keys: Res<ButtonInput<KeyCode>>,
    controls: Res<Controls>,
    mut inter: ResMut<Interact>,
    seat: Res<Seat>,
    vitals: Res<Vitals>,
    room: Res<Room>,
    net: Res<Net>,
    me: Query<&Transform, With<LocalPlayer>>,
    vehicles: Query<(&NetObject, &Transform, &VehicleBody, Option<&Velocity>), Without<LocalPlayer>>,
    mut rebirth: MessageWriter<crate::tycoon::RebirthRequest>,
    mut spawn: MessageWriter<crate::tycoon::SpawnVehicleRequest>,
) {
    let Ok(pt) = me.single() else { return };
    if let Some(s) = seat.0 {
        inter.best = Some((0.0, "Exit vehicle".into(), Action::Enter { id: s.id, seat: u32::MAX }));
    } else if vitals.alive() {
        for (o, vt, vb, _) in &vehicles {
            if o.seats.is_empty() {
                continue;
            }
            let d = pt.translation.distance(vt.translation) - vb.half.max_element();
            if d > 2.5 {
                continue;
            }
            let Some(free) = o.seats.iter().position(|&s| s == 0) else { continue };
            let label = if free == 0 { format!("Drive {}", o.def) } else { format!("Ride in {}", o.def) };
            inter.offer(d, label, Action::Enter { id: o.id, seat: free as u32 });
        }
    }
    if !(controls.captured && keys.just_pressed(KeyCode::KeyE)) {
        return;
    }
    let Some((_, _, action)) = inter.best.clone() else { return };
    match action {
        Action::Enter { id, seat: u32::MAX } => {
            let state = vehicles
                .iter()
                .find(|(o, ..)| o.id == id && o.owner == room.me)
                .map(|(o, t, _, v)| state_of(o, t, v));
            net.send(C::Release(pb::ReleaseObject { id, state }));
        }
        Action::Enter { id, seat } => net.send(C::Claim(pb::ClaimObject { id, seat })),
        Action::SpawnVehicle { def, pos, yaw } => {
            spawn.write(crate::tycoon::SpawnVehicleRequest { def, pos, yaw });
        }
        Action::Rebirth => {
            rebirth.write(crate::tycoon::RebirthRequest);
        }
    }
}

pub fn clear_interact(mut inter: ResMut<Interact>) {
    inter.best = None;
}

/// Spins rotors/propellers according to the vehicle's reported throttle.
pub fn vehicle_spinners(
    vehicles: Query<(&NetObject, &Children), With<VehicleBody>>,
    mut spinners: Query<&mut Spinner>,
) {
    for (o, children) in &vehicles {
        let speed = o.extra.get(2).copied().unwrap_or(0.0);
        for c in children.iter() {
            if let Ok(mut s) = spinners.get_mut(c) {
                s.speed = speed * 30.0;
            }
        }
    }
}
