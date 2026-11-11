//! First-person view model: a second camera that draws only the held weapon
//! and arms (so they never clip into walls), with sway, bob, recoil, aim down
//! sights, reload/pump/bolt animations, muzzle flash, shell casings and an
//! inspect pose used by the gunsmith.

use std::f32::consts::{FRAC_PI_2, PI};

use bevy::camera::visibility::RenderLayers;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy_egui::EguiGlobalSettings;

use crate::combat::{CombatFx, Fx, Loadout};
use crate::guns::{AnimKind, Armory, GunAnim, GunInfo, GunKit, Role, VM_LAYER};
use crate::objects::MotionVel;
use crate::player::{Controls, LocalPlayer, Look, Seat, Vitals};
use crate::state::{AppState, Room};
use crate::world::MainCamera;

/// Vertical field of view of the view-model camera.
pub const VM_FOV: f32 = 54.0;

#[derive(Component)]
pub struct ViewModelCam;

#[derive(Component)]
pub struct VmRig;

#[derive(Component)]
pub struct VmArms;

#[derive(Component)]
pub struct Shell {
    vel: Vec3,
    spin: Vec3,
    life: f32,
}

#[derive(Resource, Default)]
pub struct Vm {
    gun: Option<Entity>,
    sig: Option<(String, u32, u32)>,
    weapon: String,
    pub info: GunInfo,
    ads: f32,
    sprint: f32,
    inspect: f32,
    draw: f32,
    sway: Vec2,
    bob_phase: f32,
    bob_amp: f32,
    recoil: f32,
    recoil_rot: Vec2,
    last_fire: f64,
    /// Where tracers start, in world space.
    pub muzzle_world: Vec3,
    pub muzzle_dir: Vec3,
}

/// Spawns the view-model camera as a child of the main camera. The egui
/// context lives on it so the HUD draws over the weapon.
pub fn spawn_camera(commands: &mut Commands, main: Entity, egui: &mut EguiGlobalSettings) {
    egui.auto_create_primary_context = false;
    let cam = commands
        .spawn((
            Camera3d::default(),
            Camera { order: 1, clear_color: ClearColorConfig::None, ..default() },
            Projection::Perspective(PerspectiveProjection { fov: VM_FOV.to_radians(), near: 0.01, far: 20.0, ..default() }),
            RenderLayers::layer(VM_LAYER),
            ViewModelCam,
        ))
        .id();
    commands.entity(main).add_child(cam);
    let rig = commands.spawn((Transform::default(), Visibility::Hidden, VmRig)).id();
    commands.entity(cam).add_child(rig);
    // Soft key light so the weapon always reads well, whatever the sun does.
    let key = commands
        .spawn((
            PointLight { intensity: 9_000.0, range: 3.0, color: Color::srgb(1.0, 0.96, 0.9), shadow_maps_enabled: false, ..default() },
            Transform::from_xyz(0.5, 0.45, 0.25),
            RenderLayers::layer(VM_LAYER),
        ))
        .id();
    commands.entity(cam).add_child(key);
}

pub fn hide_rig(mut rig: Query<&mut Visibility, With<VmRig>>, mut vm: ResMut<Vm>) {
    for mut v in &mut rig {
        *v = Visibility::Hidden;
    }
    vm.ads = 0.0;
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn toward(x: f32, target: f32, step: f32) -> f32 {
    if x < target { (x + step).min(target) } else { (x - step).max(target) }
}

fn hip_pose(id: &str) -> (Vec3, Quat) {
    match id {
        "pistol" => (Vec3::new(0.14, -0.15, -0.38), Quat::from_rotation_y(0.06)),
        "rocket" => (Vec3::new(0.19, -0.17, -0.3), Quat::from_rotation_y(0.03)),
        "knife" => (Vec3::new(0.18, -0.19, -0.36), Quat::from_rotation_y(0.35) * Quat::from_rotation_x(0.55) * Quat::from_rotation_z(-0.35)),
        "grenade" => (Vec3::new(0.17, -0.17, -0.34), Quat::from_rotation_y(0.3)),
        "sniper" => (Vec3::new(0.17, -0.18, -0.36), Quat::from_rotation_y(0.035)),
        _ => (Vec3::new(0.17, -0.185, -0.38), Quat::from_rotation_y(0.045)),
    }
}

/// Maps a point in view-model camera space to world space, so it lines up
/// on screen with the main camera's wider or narrower field of view.
fn vm_to_world(cam: &GlobalTransform, main_fov: f32, p: Vec3) -> Vec3 {
    let k = (main_fov / 2.0).tan() / (VM_FOV.to_radians() / 2.0).tan();
    cam.transform_point(Vec3::new(p.x * k, p.y * k, p.z))
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn update_viewmodel(
    time: Res<Time>,
    mut kit: GunKit,
    mut vm: ResMut<Vm>,
    (loadout, armory, seat, vitals, controls, room, fx, gunsmith): (
        Res<Loadout>,
        Res<Armory>,
        Res<Seat>,
        Res<Vitals>,
        Res<Controls>,
        Res<Room>,
        Res<CombatFx>,
        Res<crate::gunsmith::GunsmithUi>,
    ),
    (keys, mouse, motion): (Res<ButtonInput<KeyCode>>, Res<ButtonInput<MouseButton>>, Res<AccumulatedMouseMotion>),
    mut look: ResMut<Look>,
    mut rig: Query<(Entity, &mut Transform, &mut Visibility), (With<VmRig>, Without<GunAnim>, Without<VmArms>)>,
    mut anims: Query<(&GunAnim, &mut Transform, &mut Visibility), (Without<VmRig>, Without<VmArms>)>,
    mut arms: Query<&mut Visibility, (With<VmArms>, Without<VmRig>, Without<GunAnim>)>,
    me: Query<&MotionVel, With<LocalPlayer>>,
    cams: Query<(&GlobalTransform, &Projection), With<MainCamera>>,
) {
    let Ok((rig_e, mut rig_t, mut rig_v)) = rig.single_mut() else { return };
    let dt = time.delta_secs().min(0.05);
    let now = time.elapsed_secs_f64();
    let id = loadout.current_id().to_string();
    let mods = armory.get(&id);
    let h = loadout.handling();
    let team = crate::ui::color_u32(room.team_color(room.my_team()));

    // Rebuild when the weapon, its attachments or the team change.
    let sig = (id.clone(), mods.pack(), team);
    if vm.sig.as_ref() != Some(&sig) {
        if let Some(old) = vm.gun.take() {
            kit.commands.entity(old).despawn();
        }
        let root = kit.commands.spawn((Transform::IDENTITY, Visibility::default())).id();
        kit.commands.entity(rig_e).add_child(root);
        vm.info = kit.build_gun(root, &id, mods, true, team);
        vm.gun = Some(root);
        if vm.weapon != id {
            vm.draw = 0.0;
            vm.weapon = id.clone();
        }
        vm.sig = Some(sig);
    }

    // Inputs.
    let reload = loadout.reload_progress();
    let speed = me.single().map(|m| Vec2::new(m.0.x, m.0.z).length()).unwrap_or(0.0);
    let on_foot = seat.0.is_none() && vitals.alive() && look.chase == 0.0;
    let can_ads = on_foot && controls.captured && reload.is_none() && !matches!(id.as_str(), "knife" | "grenade");
    let ads_target = if can_ads && mouse.pressed(MouseButton::Right) { 1.0 } else { 0.0 };
    vm.ads = toward(vm.ads, ads_target, dt / h.ads_time.max(0.05));
    let e = smooth(vm.ads);
    look.ads = vm.ads;
    look.zoom = 1.0 + (h.zoom - 1.0) * e;
    look.scoped = h.scoped && vm.ads > 0.96;
    let sprinting = on_foot && controls.captured && keys.pressed(KeyCode::ShiftLeft) && keys.pressed(KeyCode::KeyW) && speed > 8.0 && vm.ads < 0.1;
    vm.sprint = toward(vm.sprint, if sprinting { 1.0 } else { 0.0 }, dt * 5.0);
    let inspecting = gunsmith.open || (controls.captured && keys.pressed(KeyCode::KeyF) && vm.ads < 0.1);
    vm.inspect = toward(vm.inspect, if inspecting { 1.0 } else { 0.0 }, dt * 2.6);
    vm.draw = (vm.draw + dt * 3.2).min(1.0);

    let visible = on_foot && !look.scoped;
    let want = if visible { Visibility::Inherited } else { Visibility::Hidden };
    if *rig_v != want {
        *rig_v = want;
    }

    // Fire events: kick, flash light, casing.
    let fired = fx.fired_at > vm.last_fire;
    if fired {
        vm.last_fire = fx.fired_at;
        vm.recoil = (vm.recoil + 1.0).min(2.5);
        let kick = (h.recoil / 2.0).clamp(0.2, 2.5);
        vm.recoil_rot += Vec2::new((rand::random::<f32>() - 0.5) * 0.6, 1.0) * kick;
        if id == "grenade" || id == "rocket" {
            vm.draw = vm.draw.min(0.2);
        }
    }
    let fire_t = (now - vm.last_fire) as f32;
    let decay = 1.0 - (-dt * 12.0).exp();
    vm.recoil -= vm.recoil * decay;
    let rr = vm.recoil_rot;
    vm.recoil_rot -= rr * (1.0 - (-dt * 9.0).exp());

    // Sway follows the mouse, bob follows the feet.
    let mv = if controls.captured { motion.delta } else { Vec2::ZERO };
    let target = Vec2::new(-mv.x, -mv.y) * 0.0004;
    let target = target.clamp(Vec2::splat(-0.05), Vec2::splat(0.05));
    let s = vm.sway;
    vm.sway += (target - s) * (1.0 - (-dt * 10.0).exp());
    let amp = (speed / 7.0).min(1.5) * (1.0 - 0.85 * e);
    vm.bob_amp += (amp - vm.bob_amp) * (1.0 - (-dt * 8.0).exp());
    vm.bob_phase = (vm.bob_phase + dt * (2.0 + speed * 1.2)) % (PI * 4.0);

    // Pose.
    let info = vm.info;
    let (hip, hip_rot) = hip_pose(&id);
    let ads_pos = Vec3::new(0.0, 0.0, -info.ads_dist) - info.sight;
    let mut pos = hip.lerp(ads_pos, e);
    let mut rot = hip_rot.slerp(Quat::IDENTITY, e);
    let t = time.elapsed_secs();
    pos += Vec3::new((t * 0.9).sin() * 0.0015, (t * 1.7).sin() * 0.002, 0.0) * (1.0 - 0.85 * e);
    let ph = vm.bob_phase;
    pos += Vec3::new(ph.sin() * 0.011, -(ph * 2.0).sin().abs() * 0.01, 0.0) * vm.bob_amp;
    rot *= Quat::from_rotation_z(ph.sin() * 0.02 * vm.bob_amp);
    let sw = vm.sway * (1.0 - 0.7 * e);
    pos += Vec3::new(sw.x, sw.y, 0.0) * 0.5;
    rot *= Quat::from_euler(EulerRot::YXZ, sw.x * 1.4, -sw.y * 1.4, sw.x * 2.0);
    let kick_back = match id.as_str() {
        "shotgun" | "sniper" | "rocket" => 0.07,
        "pistol" => 0.03,
        _ => 0.025,
    };
    pos.z += vm.recoil * kick_back * (1.0 - 0.4 * e);
    pos.y += vm.recoil * 0.004;
    rot *= Quat::from_rotation_x(vm.recoil_rot.y * 0.06 * (1.0 - 0.6 * e)) * Quat::from_rotation_y(vm.recoil_rot.x * 0.05);
    let sp = smooth(vm.sprint);
    pos += Vec3::new(-0.05, -0.04, 0.03) * sp;
    rot *= Quat::from_euler(EulerRot::YXZ, 0.6 * sp, -0.3 * sp, 0.3 * sp);
    if let Some(p) = reload {
        let k = (p * PI).sin();
        pos += Vec3::new(-0.03, -0.05, 0.03) * k;
        rot *= Quat::from_euler(EulerRot::YXZ, 0.25 * k, 0.3 * k, 0.6 * k);
    }
    if id == "knife" && fire_t < 0.35 {
        let s = (fire_t / 0.35 * PI).sin();
        pos += Vec3::new(-0.14 * s, 0.05 * s, -0.12 * s);
        rot *= Quat::from_euler(EulerRot::YXZ, 0.9 * s, -0.4 * s, -0.7 * s);
    }
    if id == "grenade" && fire_t < 0.3 {
        let s = (fire_t / 0.3 * PI).sin();
        pos += Vec3::new(-0.06, 0.14, -0.2) * s;
    }
    let d = 1.0 - smooth(vm.draw);
    pos.y -= 0.28 * d;
    rot *= Quat::from_rotation_x(-0.8 * d);
    // Inspect: turn the weapon side-on, centred in the free space right of the gunsmith panel.
    let ie = smooth(vm.inspect);
    if ie > 0.0 {
        let len = info.zmax - info.zmin;
        let wobble = (t * 0.6).sin() * 0.18;
        let irot = Quat::from_rotation_y(-FRAC_PI_2 + wobble) * Quat::from_rotation_x((t * 0.45).sin() * 0.06);
        let centre = Vec3::new(0.0, 0.02, (info.zmin + info.zmax) / 2.0);
        let dist = 0.3 + len * 1.15;
        let x = if gunsmith.open { dist * 0.04 } else { 0.0 };
        let ipos = Vec3::new(x, -0.03 * dist, -dist) - irot * centre;
        pos = pos.lerp(ipos, ie);
        rot = rot.slerp(irot, ie);
    }
    rig_t.translation = pos;
    rig_t.rotation = rot;
    for mut v in &mut arms {
        let want = if ie > 0.3 { Visibility::Hidden } else { Visibility::Inherited };
        if *v != want {
            *v = want;
        }
    }

    // Moving parts.
    let ammo = loadout.current_ammo();
    for (a, mut tr, mut vis) in &mut anims {
        if !a.vm {
            continue;
        }
        let mut out = a.base;
        let mut show = true;
        match a.kind {
            AnimKind::Mag => {
                if let Some(p) = reload {
                    let drop = if p < 0.1 {
                        0.0
                    } else if p < 0.35 {
                        smooth((p - 0.1) / 0.25)
                    } else if p < 0.55 {
                        1.0
                    } else {
                        1.0 - smooth((p - 0.55) / 0.25)
                    };
                    out.translation += Vec3::new(0.0, -0.2, 0.06) * drop;
                    show = drop < 0.97;
                }
            }
            AnimKind::Pump => {
                if (0.12..0.5).contains(&fire_t) {
                    out.translation.z += 0.075 * ((fire_t - 0.12) / 0.38 * PI).sin();
                }
            }
            AnimKind::Slide => {
                if fire_t < 0.08 {
                    out.translation.z += 0.03 * (1.0 - fire_t / 0.08);
                }
            }
            AnimKind::Bolt => {
                if (0.25..0.9).contains(&fire_t) {
                    let k = ((fire_t - 0.25) / 0.65 * PI).sin();
                    out.rotation *= Quat::from_rotation_z(1.1 * smooth(k * 2.0));
                    out.translation.z += 0.07 * smooth(k * 2.0 - 1.0);
                }
            }
            AnimKind::RocketTip => show = ammo > 0,
            AnimKind::Flash => {
                show = fire_t < 0.05 && h.flash > 0.0 && visible;
                if show {
                    let sc = h.flash * (0.8 + rand::random::<f32>() * 0.5);
                    out.scale = Vec3::splat(sc);
                    out.rotation = Quat::from_rotation_z(rand::random::<f32>() * PI);
                }
            }
        }
        *tr = out;
        let want = if show { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
    }

    // World-space muzzle for tracers, plus the flash light and the casing.
    let Ok((cam_gt, proj)) = cams.single() else { return };
    let main_fov = match proj {
        Projection::Perspective(p) => p.fov,
        _ => VM_FOV.to_radians(),
    };
    let rig_tr = Transform { translation: pos, rotation: rot, ..default() };
    vm.muzzle_world = vm_to_world(cam_gt, main_fov, rig_tr.transform_point(info.muzzle));
    vm.muzzle_dir = cam_gt.rotation() * (rot * Vec3::NEG_Z);
    if fired && visible {
        if h.flash > 0.0 && !h.suppressed {
            kit.commands.spawn((
                PointLight { intensity: 60_000.0 * h.flash, range: 9.0, color: Color::srgb(1.0, 0.75, 0.4), shadow_maps_enabled: false, ..default() },
                Transform::from_translation(vm.muzzle_world),
                RenderLayers::from_layers(&[0, VM_LAYER]),
                Fx { life: 0.05, grow: Vec3::ZERO },
            ));
        }
        if let Some(ej) = info.eject {
            let at = vm_to_world(cam_gt, main_fov, rig_tr.transform_point(ej));
            let right = cam_gt.right().as_vec3();
            let up = cam_gt.up().as_vec3();
            let r = || rand::random::<f32>();
            let shotgun = id == "shotgun";
            let (radius, len) = match id.as_str() {
                "pistol" => (0.0045, 0.02),
                "shotgun" => (0.009, 0.06),
                "sniper" => (0.006, 0.07),
                _ => (0.005, 0.045),
            };
            let cx = crate::guns::Ctx { skin: 0, reticle: 0, factory: 0, vm: false, team: 0x9E1E24 };
            let mat = kit.mat(if shotgun { Role::Cuff } else { Role::Brass }, &cx);
            let mesh = kit.shell_mesh(radius, len);
            kit.commands.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(mat),
                Transform::from_translation(at).with_rotation(Quat::from_rotation_z(FRAC_PI_2) * Quat::from_rotation_x(r() * 3.0)),
                Shell { vel: right * (1.8 + r()) + up * (1.4 + r()) - cam_gt.forward().as_vec3() * 0.4, spin: Vec3::new(r() * 20.0, r() * 30.0, r() * 10.0), life: 1.2 },
                DespawnOnExit(AppState::InGame),
            ));
        }
    }
}

pub fn update_shells(time: Res<Time>, mut commands: Commands, mut q: Query<(Entity, &mut Shell, &mut Transform)>) {
    let dt = time.delta_secs().min(0.05);
    for (e, mut s, mut t) in &mut q {
        s.life -= dt;
        if s.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        s.vel.y -= 9.8 * dt;
        t.translation += s.vel * dt;
        t.rotate_local(Quat::from_euler(EulerRot::XYZ, s.spin.x * dt, s.spin.y * dt, s.spin.z * dt));
    }
}
