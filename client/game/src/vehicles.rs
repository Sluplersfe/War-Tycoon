//! Driving. Cars and tanks use Rapier's ray-cast vehicle controller; the
//! helicopter and plane use arcade velocity control. Controllers exist only
//! while this client owns the vehicle (that is, while it is the driver).

use bevy::prelude::*;
use bevy_rapier3d::prelude::*;
use frontline_sim::defs::VehicleClass;

use crate::art::{BarrelPart, TurretPart};
use crate::objects::{NetObject, VehicleBody};
use crate::player::{Controls, Look, Seat};
use crate::state::Room;

const CAR_SPEED: f32 = 30.0;
const CAR_ACCEL: f32 = 16.0;
const TANK_SPEED: f32 = 11.0;
const TANK_ACCEL: f32 = 6.0;
const TANK_TURN: f32 = 0.9;
const TURRET_RATE: f32 = 1.6;
const HELI_SPEED: f32 = 24.0;
const HELI_CLIMB: f32 = 11.0;
const PLANE_MAX: f32 = 62.0;
const PLANE_STALL: f32 = 24.0;

/// Index into `NetObject.extra` of the vehicle fields.
pub const X_TURRET_YAW: usize = 0;
pub const X_TURRET_PITCH: usize = 1;
pub const X_ROTOR: usize = 2;
pub const X_THROTTLE: usize = 3;

fn wheel_tuning(mass: f32, stiffness: f32, slip: f32, side: f32) -> WheelTuning {
    WheelTuning {
        suspension_stiffness: stiffness,
        suspension_compression: 2.3,
        suspension_damping: 4.4,
        max_suspension_travel: 1.0,
        side_friction_stiffness: side,
        friction_slip: slip,
        max_suspension_force: mass * 60.0,
    }
}

pub fn attach_controller(commands: &mut Commands, e: Entity, _def: &str, vb: &VehicleBody) {
    let mut ec = commands.entity(e);
    match vb.class {
        VehicleClass::Car => {
            let tune = wheel_tuning(vb.mass, 26.0, 11.0, 1.0);
            let wheels = [(-1.0, 1.35), (1.0, 1.35), (-1.0, -1.35), (1.0, -1.35)]
                .into_iter()
                .map(|(x, z)| VehicleWheel::new(Vec3::new(x, 0.0, z), -Vec3::Y, -Vec3::X, 0.4, 0.45, tune))
                .collect();
            ec.insert((
                RayCastVehicleController { wheels, index_forward_axis: 2, ..default() },
                Damping { linear_damping: 0.05, angular_damping: 1.2 },
            ));
        }
        VehicleClass::Tank => {
            let tune = wheel_tuning(vb.mass, 22.0, 14.0, 1.5);
            let mut wheels = Vec::new();
            for x in [-1.45, 1.45] {
                for z in [-2.4, 0.0, 2.4] {
                    wheels.push(VehicleWheel::new(Vec3::new(x, -0.1, z), -Vec3::Y, -Vec3::X, 0.6, 0.5, tune));
                }
            }
            ec.insert((
                RayCastVehicleController { wheels, index_forward_axis: 2, ..default() },
                Damping { linear_damping: 0.1, angular_damping: 3.0 },
            ));
        }
        VehicleClass::Heli => {
            ec.insert((
                LockedAxes::ROTATION_LOCKED_X | LockedAxes::ROTATION_LOCKED_Z,
                GravityScale(0.0),
                Damping { linear_damping: 0.0, angular_damping: 2.0 },
            ));
        }
        VehicleClass::Plane => {
            ec.insert((
                GravityScale(1.0),
                Damping { linear_damping: 0.0, angular_damping: 2.0 },
                Friction { coefficient: 0.0, combine_rule: CoefficientCombineRule::Min },
            ));
        }
        VehicleClass::Prop => {}
    }
}

pub fn detach_controller(commands: &mut Commands, e: Entity) {
    commands
        .entity(e)
        .remove::<(RayCastVehicleController, LockedAxes, GravityScale, Damping, Friction)>();
}

fn wrap(a: f32) -> f32 {
    let mut a = a % std::f32::consts::TAU;
    if a > std::f32::consts::PI {
        a -= std::f32::consts::TAU;
    } else if a < -std::f32::consts::PI {
        a += std::f32::consts::TAU;
    }
    a
}

fn axis(keys: &ButtonInput<KeyCode>, pos: KeyCode, neg: KeyCode) -> f32 {
    (keys.pressed(pos) as i32 - keys.pressed(neg) as i32) as f32
}

/// Applies the driver's input to the vehicle they own.
#[allow(clippy::too_many_arguments)]
pub fn drive(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    controls: Res<Controls>,
    look: Res<Look>,
    seat: Res<Seat>,
    room: Res<Room>,
    mut q: Query<
        (
            &mut NetObject,
            &VehicleBody,
            &mut Transform,
            &mut Velocity,
            Option<&mut RayCastVehicleController>,
            Option<&mut GravityScale>,
        ),
        With<RigidBody>,
    >,
) {
    let Some(s) = seat.driving() else { return };
    let Ok((mut obj, vb, mut t, mut vel, ctrl, grav)) = q.get_mut(s.vehicle) else { return };
    if obj.owner != room.me {
        return;
    }
    let dt = time.delta_secs().min(0.05);
    if obj.extra.len() < 4 {
        obj.extra.resize(4, 0.0);
    }
    let live = controls.captured;
    let throttle = if live { axis(&keys, KeyCode::KeyW, KeyCode::KeyS) } else { 0.0 };
    let steer = if live { axis(&keys, KeyCode::KeyA, KeyCode::KeyD) } else { 0.0 };
    let space = live && keys.pressed(KeyCode::Space);

    if live && keys.just_pressed(KeyCode::KeyR) && matches!(vb.class, VehicleClass::Car | VehicleClass::Tank) {
        let (yaw, _, _) = t.rotation.to_euler(EulerRot::YXZ);
        t.rotation = Quat::from_rotation_y(yaw);
        t.translation.y += 2.0;
        vel.linear = Vec3::ZERO;
        vel.angular = Vec3::ZERO;
    }

    let (hull_yaw, _, _) = t.rotation.to_euler(EulerRot::YXZ);
    if std::env::var("FF_DEBUG").is_ok() && (time.elapsed_secs() * 2.0) as u32 != ((time.elapsed_secs() - dt) * 2.0) as u32 {
        info!("drive {:?} pos={:?} yaw={:.2} lin={:?} thr={throttle} live={live}", vb.class, t.translation, hull_yaw, vel.linear);
    }
    match vb.class {
        VehicleClass::Car | VehicleClass::Tank => {
            let Some(mut c) = ctrl else { return };
            let tank = vb.class == VehicleClass::Tank;
            let (max, accel) = if tank { (TANK_SPEED, TANK_ACCEL) } else { (CAR_SPEED, CAR_ACCEL) };
            let n = c.wheels.len() as f32;
            let speed = c.current_vehicle_speed;
            let drive_on = throttle != 0.0 && (speed * throttle < max);
            let braking = space || throttle == 0.0 || speed * throttle < -1.5;
            let force = if drive_on { throttle * vb.mass * accel / n } else { 0.0 };
            let brake = if braking { vb.mass * if space { 0.08 } else { 0.03 } } else { 0.0 };
            let steer_angle = if tank { 0.0 } else { steer * (0.55 - 0.3 * (speed.abs() / max).min(1.0)) };
            for (i, w) in c.wheels.iter_mut().enumerate() {
                w.engine_force = force;
                w.brake = brake;
                if !tank && i < 2 {
                    w.steering = steer_angle;
                }
            }
            if tank {
                if c.any_wheel_in_contact() {
                    let want = steer * TANK_TURN * if speed < -0.5 { -1.0 } else { 1.0 };
                    vel.angular.y += (want - vel.angular.y) * (6.0 * dt).min(1.0);
                }
                // Turret follows the camera at a limited rate.
                let want_yaw = wrap(look.yaw - hull_yaw);
                let cur = obj.extra[X_TURRET_YAW];
                let step = (TURRET_RATE * dt).max(0.0);
                obj.extra[X_TURRET_YAW] = wrap(cur + wrap(want_yaw - cur).clamp(-step, step));
                let want_pitch = look.pitch.clamp(-0.12, 0.6);
                let cp = obj.extra[X_TURRET_PITCH];
                obj.extra[X_TURRET_PITCH] = cp + (want_pitch - cp).clamp(-step, step);
            }
            obj.extra[X_ROTOR] = 0.0;
        }
        VehicleClass::Heli => {
            let up = if live { axis(&keys, KeyCode::Space, KeyCode::ControlLeft) } else { 0.0 };
            let diff = wrap(look.yaw - hull_yaw);
            vel.angular = Vec3::new(0.0, (diff * 3.0).clamp(-1.8, 1.8), 0.0);
            let yaw = Quat::from_rotation_y(hull_yaw);
            let want = yaw * Vec3::new(steer * HELI_SPEED * 0.6, 0.0, throttle * HELI_SPEED) + Vec3::Y * up * HELI_CLIMB;
            let k = (2.5 * dt).min(1.0);
            let cur = vel.linear;
            vel.linear = cur + (want - cur) * k;
            obj.extra[X_ROTOR] = 1.0;
        }
        VehicleClass::Plane => {
            let thr = (obj.extra[X_THROTTLE] + throttle * 0.45 * dt).clamp(0.0, 1.0);
            obj.extra[X_THROTTLE] = thr;
            let fwd = t.rotation * Vec3::Z;
            let cur = vel.linear.dot(fwd).max(0.0);
            let target = thr * PLANE_MAX;
            let speed = cur + (target - cur) * (0.6 * dt).min(1.0) - if space { 20.0 * dt } else { 0.0 };
            let speed = speed.max(0.0);
            let flying = speed >= PLANE_STALL;
            if let Some(mut g) = grav {
                g.0 = if flying { 0.0 } else { 1.0 };
            }
            let pitch = if flying { look.pitch.clamp(-0.8, 0.8) } else { 0.0 };
            let bank = steer * 0.7;
            let want_rot = Quat::from_euler(EulerRot::YXZ, look.yaw, -pitch, 0.0) * Quat::from_rotation_z(bank);
            let delta = want_rot * t.rotation.inverse();
            let (ax, mut ang) = delta.to_axis_angle();
            if ang > std::f32::consts::PI {
                ang -= std::f32::consts::TAU;
            }
            let w = (ax * ang * 2.5).clamp_length_max(2.2);
            vel.angular = if w.is_finite() { w } else { Vec3::ZERO };
            if flying {
                vel.linear = fwd * speed;
            } else {
                let flat = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero() * speed;
                vel.linear = Vec3::new(flat.x, vel.linear.y, flat.z);
            }
            obj.extra[X_ROTOR] = 0.25 + thr * 0.75;
        }
        VehicleClass::Prop => {}
    }
}

/// Aims turrets from the synchronised `extra` fields (all clients).
pub fn animate_turrets(
    vehicles: Query<(&NetObject, &Children), With<VehicleBody>>,
    mut turrets: Query<(&mut Transform, Option<&Children>), (With<TurretPart>, Without<BarrelPart>)>,
    mut barrels: Query<&mut Transform, (With<BarrelPart>, Without<TurretPart>)>,
) {
    for (obj, children) in &vehicles {
        let yaw = obj.extra.get(X_TURRET_YAW).copied().unwrap_or(0.0);
        let pitch = obj.extra.get(X_TURRET_PITCH).copied().unwrap_or(0.0);
        for c in children.iter() {
            if let Ok((mut tt, tc)) = turrets.get_mut(c) {
                tt.rotation = Quat::from_rotation_y(yaw);
                if let Some(tc) = tc {
                    for b in tc.iter() {
                        if let Ok(mut bt) = barrels.get_mut(b) {
                            bt.rotation = Quat::from_rotation_x(-pitch);
                        }
                    }
                }
            }
        }
    }
}

/// Muzzle position and firing direction for a vehicle's primary weapon.
pub fn muzzle(class: VehicleClass, t: &Transform, extra: &[f32], aim_dir: Vec3) -> (Vec3, Vec3) {
    match class {
        VehicleClass::Tank => {
            let yaw = extra.get(X_TURRET_YAW).copied().unwrap_or(0.0);
            let pitch = extra.get(X_TURRET_PITCH).copied().unwrap_or(0.0);
            let local = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch);
            let dir = t.rotation * local * Vec3::Z;
            let origin = t.translation + t.rotation * Vec3::new(0.0, 0.85, -0.3) + t.rotation * local * Vec3::new(0.0, 0.0, 5.3);
            (origin, dir)
        }
        VehicleClass::Plane => (t.translation + t.rotation * Vec3::new(0.0, 0.0, 4.5), aim_dir),
        VehicleClass::Heli => (t.translation + t.rotation * Vec3::new(0.0, -0.8, 4.5), aim_dir),
        _ => (t.translation + Vec3::Y * 1.2, aim_dir),
    }
}
