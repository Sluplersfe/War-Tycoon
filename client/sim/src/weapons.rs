//! Weapon definitions and the per-weapon fire/reload state machine.

use serde::Deserialize;

pub const WEAPONS_RON: &str = include_str!("../../assets/tycoon/weapons.ron");

#[derive(Debug, Clone, Deserialize)]
pub enum WeaponKind {
    Melee { range: f32 },
    Hitscan { range: f32, spread: f32, pellets: u32 },
    Projectile { speed: f32, gravity: f32, splash: f32 },
}

#[derive(Debug, Clone, Deserialize)]
pub struct WeaponDef {
    pub id: String,
    pub name: String,
    pub slot: u32,
    pub kind: WeaponKind,
    pub damage: f32,
    pub rate: f32,
    pub mag: u32,
    pub reload: f32,
    pub auto: bool,
}

pub fn load() -> Vec<WeaponDef> {
    ron::from_str(WEAPONS_RON).expect("assets/tycoon/weapons.ron")
}

/// Splash damage falls off linearly to 20% at the edge.
pub fn splash_damage(def: &WeaponDef, dist: f32) -> f32 {
    match def.kind {
        WeaponKind::Projectile { splash, .. } if dist <= splash => def.damage * (1.0 - 0.8 * dist / splash),
        _ => 0.0,
    }
}

#[derive(Debug, Clone)]
pub struct WeaponState {
    pub ammo: u32,
    pub cooldown: f32,
    pub reloading: f32,
}

impl WeaponState {
    pub fn new(def: &WeaponDef) -> Self {
        Self { ammo: def.mag, cooldown: 0.0, reloading: 0.0 }
    }

    pub fn tick(&mut self, def: &WeaponDef, dt: f32) {
        self.cooldown = (self.cooldown - dt).max(0.0);
        if self.reloading > 0.0 {
            self.reloading -= dt;
            if self.reloading <= 0.0 {
                self.reloading = 0.0;
                self.ammo = def.mag;
            }
        }
    }

    /// Melee and magazine-less weapons never run dry. Grenades (reload 0) do
    /// not reload on their own.
    pub fn try_fire(&mut self, def: &WeaponDef) -> bool {
        if self.cooldown > 0.0 || self.reloading > 0.0 {
            return false;
        }
        if def.mag > 0 {
            if self.ammo == 0 {
                self.start_reload(def);
                return false;
            }
            self.ammo -= 1;
        }
        self.cooldown = 1.0 / def.rate.max(0.01);
        if def.mag > 0 && self.ammo == 0 {
            self.start_reload(def);
        }
        true
    }

    pub fn start_reload(&mut self, def: &WeaponDef) {
        if def.reload > 0.0 && self.ammo < def.mag && self.reloading <= 0.0 {
            self.reloading = def.reload;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(id: &str) -> WeaponDef {
        load().into_iter().find(|w| w.id == id).unwrap()
    }

    #[test]
    fn fire_and_reload() {
        let p = get("pistol");
        let mut s = WeaponState::new(&p);
        assert!(s.try_fire(&p));
        assert!(!s.try_fire(&p), "cooldown");
        for _ in 0..11 {
            s.tick(&p, 1.0);
            assert!(s.try_fire(&p));
        }
        assert_eq!(s.ammo, 0);
        assert!(s.reloading > 0.0, "auto reload on empty");
        s.tick(&p, 2.0);
        assert_eq!(s.ammo, p.mag);
    }

    #[test]
    fn grenades_do_not_refill() {
        let g = get("grenade");
        let mut s = WeaponState::new(&g);
        assert!(s.try_fire(&g));
        s.tick(&g, 5.0);
        assert!(s.try_fire(&g));
        s.tick(&g, 5.0);
        assert!(!s.try_fire(&g));
    }

    #[test]
    fn splash() {
        let r = get("rocket");
        assert_eq!(splash_damage(&r, 0.0), r.damage);
        assert!(splash_damage(&r, 5.9) > 0.0);
        assert_eq!(splash_damage(&r, 7.0), 0.0);
    }
}
