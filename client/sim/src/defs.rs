//! Shared map and vehicle definitions (`shared/*.json`, also read by the Go server).

use std::collections::HashMap;

use glam::{Quat, Vec3};
use serde::Deserialize;

pub const MAP_JSON: &str = include_str!("../../../shared/map.json");
pub const VEHICLES_JSON: &str = include_str!("../../../shared/vehicles.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Aabb {
    pub name: String,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub fn center(&self) -> Vec3 {
        (Vec3::from(self.min) + Vec3::from(self.max)) * 0.5
    }
    pub fn half_extents(&self) -> Vec3 {
        (Vec3::from(self.max) - Vec3::from(self.min)) * 0.5
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Plots {
    pub count: usize,
    pub radius: f32,
    pub size: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Trees {
    pub center: [f32; 3],
    pub radius: f32,
    pub count: usize,
    pub seed: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MapDef {
    pub name: String,
    pub half_size: f32,
    pub plots: Plots,
    pub statics: Vec<Aabb>,
    pub runway: Aabb2,
    pub trees: Trees,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Aabb2 {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VehicleClass {
    Car,
    Tank,
    Heli,
    Plane,
    Prop,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VehicleDef {
    pub class: VehicleClass,
    pub seats: usize,
    pub half_extents: [f32; 3],
    pub mass: f32,
    pub health: f32,
}

impl MapDef {
    pub fn load() -> Self {
        serde_json::from_str(MAP_JSON).expect("shared/map.json")
    }

    /// Plot centre and yaw; rotating local +Z by yaw points at the island centre.
    /// Must match `world.Plot` in the Go server.
    pub fn plot(&self, i: usize) -> (Vec3, f32) {
        let a = i as f32 * std::f32::consts::TAU / self.plots.count as f32;
        let r = self.plots.radius;
        (Vec3::new(r * a.sin(), 0.0, r * a.cos()), a + std::f32::consts::PI)
    }

    pub fn plot_to_world(&self, i: usize, local: Vec3) -> Vec3 {
        let (c, yaw) = self.plot(i);
        c + Quat::from_rotation_y(yaw) * local
    }

    pub fn plot_spawn(&self, i: usize) -> Vec3 {
        self.plot_to_world(i, Vec3::new(0.0, 1.5, -20.0))
    }

    /// Which plot (if any) contains a world position.
    pub fn plot_at(&self, p: Vec3) -> Option<usize> {
        (0..self.plots.count).find(|&i| {
            let (c, yaw) = self.plot(i);
            let l = Quat::from_rotation_y(-yaw) * (p - c);
            l.x.abs() <= self.plots.size / 2.0 && l.z.abs() <= self.plots.size / 2.0
        })
    }
}

pub fn load_vehicles() -> HashMap<String, VehicleDef> {
    serde_json::from_str(VEHICLES_JSON).expect("shared/vehicles.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plots_face_center() {
        let m = MapDef::load();
        for i in 0..m.plots.count {
            let (c, yaw) = m.plot(i);
            let fwd = Quat::from_rotation_y(yaw) * Vec3::Z;
            assert!(fwd.dot(-c.normalize()) > 0.999, "plot {i} faces the centre");
            assert_eq!(m.plot_at(c), Some(i));
            assert_eq!(m.plot_at(m.plot_spawn(i)), Some(i));
        }
        assert_eq!(m.plot_at(Vec3::ZERO), None);
    }

    #[test]
    fn matches_server_spawn_formula() {
        // Go: x + (lx*c + lz*s), z + (-lx*s + lz*c) with lx=0, lz=-20.
        let m = MapDef::load();
        let (c, yaw) = m.plot(3);
        let want = Vec3::new(c.x - 20.0 * yaw.sin(), 1.5, c.z - 20.0 * yaw.cos());
        assert!(m.plot_spawn(3).distance(want) < 1e-3);
    }

    #[test]
    fn vehicles_parse() {
        let v = load_vehicles();
        assert_eq!(v["jeep"].class, VehicleClass::Car);
        assert!(v["tank"].seats >= 1);
    }
}
