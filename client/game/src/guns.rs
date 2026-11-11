//! Weapon models, attachments, finishes (skins) and the armory that stores
//! each weapon's customization.
//!
//! Models are built at real-world scale from many small parts. Gun-local
//! space matches a camera: forward is -Z, right is +X, up is +Y, and the
//! origin sits at the firing hand on the grip. Attachments are free: they
//! unlock with rank (and a couple of finishes with rebirths), never money.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::ecs::system::SystemParam;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use frontline_sim::weapons::{WeaponDef, WeaponKind};
use serde::{Deserialize, Serialize};

use crate::art::HeldWeapon;
use crate::combat::Loadout;
use crate::net::config_dir;
use crate::objects::NetObject;
use crate::player::{LocalPlayer, Look};
use crate::state::GameData;

/// Render layer of the first-person view model.
pub const VM_LAYER: usize = 1;

// ---------------------------------------------------------------- catalog

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Optic,
    Muzzle,
    Under,
    Mag,
    Stock,
}

impl Slot {
    pub const ALL: [Slot; 5] = [Slot::Optic, Slot::Muzzle, Slot::Under, Slot::Mag, Slot::Stock];
    pub fn label(self) -> &'static str {
        match self {
            Slot::Optic => "Optic",
            Slot::Muzzle => "Muzzle",
            Slot::Under => "Underbarrel",
            Slot::Mag => "Magazine",
            Slot::Stock => "Stock",
        }
    }
}

/// Multipliers an attachment applies.
#[derive(Clone, Copy, Debug)]
pub struct Mods {
    pub spread: f32,
    pub recoil: f32,
    pub ads: f32,
    pub mag: f32,
    pub reload: f32,
    pub range: f32,
    pub hip: f32,
}

const M: Mods = Mods { spread: 1.0, recoil: 1.0, ads: 1.0, mag: 1.0, reload: 1.0, range: 1.0, hip: 1.0 };

pub struct Attachment {
    pub name: &'static str,
    pub desc: &'static str,
    pub rank: u32,
    pub fx: Mods,
}

const fn att(name: &'static str, desc: &'static str, rank: u32, fx: Mods) -> Attachment {
    Attachment { name, desc, rank, fx }
}

pub const OPTICS: [Attachment; 5] = [
    att("Iron Sights", "Factory front post and rear aperture. 1.35x.", 0, M),
    att("Red Dot", "Open reflex sight with a crisp dot. 1.6x.", 0, Mods { ads: 1.03, ..M }),
    att("Holographic", "Ring-and-dot window sight. 1.75x, steadier aim.", 1, Mods { spread: 0.92, ads: 1.06, ..M }),
    att("Tactical 3x", "Mid-range scope. 3x zoom, slower to raise.", 2, Mods { spread: 0.85, ads: 1.18, ..M }),
    att("Marksman 8x", "Long-range scope. 8x zoom, slowest to raise.", 3, Mods { spread: 0.8, ads: 1.35, ..M }),
];
const OPTIC_ZOOM: [f32; 5] = [1.35, 1.6, 1.75, 3.0, 8.0];

pub const MUZZLES: [Attachment; 5] = [
    att("Standard", "Factory muzzle.", 0, M),
    att("Flash Hider", "Hides most of the muzzle flash, slightly less kick.", 0, Mods { recoil: 0.94, ..M }),
    att("Compensator", "Vents gas upward to fight muzzle climb.", 1, Mods { recoil: 0.82, ..M }),
    att("Suppressor", "Quiet shots and almost no flash. Shorter range.", 2, Mods { recoil: 0.9, range: 0.85, ..M }),
    att("Muzzle Brake", "Huge flash, much less kick, a bit heavier.", 3, Mods { recoil: 0.7, ads: 1.06, ..M }),
];

pub const UNDERS: [Attachment; 4] = [
    att("None", "Clean underside.", 0, M),
    att("Vertical Grip", "Less recoil and tighter grouping.", 1, Mods { recoil: 0.85, spread: 0.9, ads: 1.04, ..M }),
    att("Angled Grip", "Much faster to aim down sights.", 1, Mods { ads: 0.82, recoil: 0.95, ..M }),
    att("Laser Sight", "Far better hip-fire accuracy.", 2, Mods { hip: 0.6, ..M }),
];

pub const MAGS: [Attachment; 4] = [
    att("Standard", "Factory magazine.", 0, M),
    att("Extended", "+50% capacity, slightly slower reload.", 1, Mods { mag: 1.5, reload: 1.12, ads: 1.04, ..M }),
    att("Fast Mag", "Coupled magazines: much faster reload.", 2, Mods { reload: 0.72, ..M }),
    att("Drum", "+150% capacity, slow reload, heavy.", 4, Mods { mag: 2.5, reload: 1.5, ads: 1.15, ..M }),
];

pub const STOCKS: [Attachment; 4] = [
    att("Standard", "Factory stock.", 0, M),
    att("Lightweight", "Faster to aim, a touch more kick.", 1, Mods { ads: 0.82, recoil: 1.12, ..M }),
    att("Heavy", "Soaks up recoil, slower to aim.", 2, Mods { recoil: 0.78, ads: 1.12, ..M }),
    att("Precision", "Adjustable cheek rest: tighter and steadier.", 3, Mods { spread: 0.85, recoil: 0.9, ads: 1.08, ..M }),
];

pub fn table(slot: Slot) -> &'static [Attachment] {
    match slot {
        Slot::Optic => &OPTICS,
        Slot::Muzzle => &MUZZLES,
        Slot::Under => &UNDERS,
        Slot::Mag => &MAGS,
        Slot::Stock => &STOCKS,
    }
}

/// Which attachments a weapon accepts in a slot (first entry is the factory part).
pub fn options(weapon: &str, slot: Slot) -> &'static [u8] {
    use Slot::*;
    match (weapon, slot) {
        ("rifle", Optic) | ("rifle", Muzzle) | ("smg", Muzzle) => &[0, 1, 2, 3, 4],
        ("rifle", _) | ("smg", Under) | ("smg", Mag) | ("smg", Stock) => &[0, 1, 2, 3],
        ("smg", Optic) => &[0, 1, 2, 3],
        ("pistol", Optic) => &[0, 1],
        ("pistol", Muzzle) => &[0, 2, 3],
        ("pistol", Under) => &[0, 3],
        ("pistol", Mag) => &[0, 1, 2],
        ("shotgun", Optic) => &[0, 1, 2],
        ("shotgun", Muzzle) => &[0, 1, 4],
        ("shotgun", Under) => &[0, 1, 3],
        ("shotgun", Mag) => &[0, 1],
        ("shotgun", Stock) => &[0, 1, 2],
        ("sniper", Optic) => &[4, 3, 1],
        ("sniper", Muzzle) => &[0, 3, 4],
        ("sniper", Under) => &[0, 1],
        ("sniper", Mag) => &[0, 1, 2],
        ("sniper", Stock) => &[0, 2, 3],
        ("rocket", Optic) => &[0, 3],
        _ => &[],
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    Factory,
    Solid,
    Blobs,
    Digital,
    Tiger,
    Carbon,
    Hazard,
    Damascus,
    Grid,
    Cracks,
}

pub struct Skin {
    pub name: &'static str,
    pub pattern: Pattern,
    pub colors: [u32; 4],
    pub metallic: f32,
    pub rough: f32,
    pub glow: f32,
    pub rank: u32,
    pub rebirths: u32,
}

const fn skin(name: &'static str, pattern: Pattern, colors: [u32; 4], metallic: f32, rough: f32, glow: f32, rank: u32, rebirths: u32) -> Skin {
    Skin { name, pattern, colors, metallic, rough, glow, rank, rebirths }
}

pub const SKINS: [Skin; 17] = [
    skin("Factory", Pattern::Factory, [0x2B2D30, 0x3A3D40, 0, 0], 0.1, 0.6, 0.0, 0, 0),
    skin("Olive Drab", Pattern::Solid, [0x5E6B3A, 0x454E2C, 0, 0], 0.05, 0.75, 0.0, 0, 0),
    skin("Woodland", Pattern::Blobs, [0x5A6B34, 0x4A3520, 0x2A3A1E, 0x1A1A16], 0.05, 0.8, 0.0, 1, 0),
    skin("Desert", Pattern::Blobs, [0xC9B07A, 0xA88A55, 0x8C6E44, 0xE0CFA0], 0.05, 0.8, 0.0, 1, 0),
    skin("Arctic", Pattern::Blobs, [0xE6EAEE, 0xB8C0C8, 0x8A949E, 0x5E666E], 0.05, 0.7, 0.0, 2, 0),
    skin("Urban Digital", Pattern::Digital, [0x8A8E92, 0x5E6266, 0x3A3D40, 0xB4B8BC], 0.05, 0.7, 0.0, 2, 0),
    skin("Tiger Stripe", Pattern::Tiger, [0x6E7A3C, 0x22200F, 0x8A6A3A, 0], 0.05, 0.75, 0.0, 3, 0),
    skin("Midnight", Pattern::Solid, [0x1C2433, 0x2C3850, 0, 0], 0.35, 0.4, 0.0, 3, 0),
    skin("Crimson", Pattern::Solid, [0x9E1E24, 0x2A2A2A, 0, 0], 0.35, 0.4, 0.0, 4, 0),
    skin("Cobalt", Pattern::Solid, [0x2456B8, 0xC0C6CC, 0, 0], 0.45, 0.3, 0.0, 4, 0),
    skin("Hazard", Pattern::Hazard, [0xE8A317, 0x1E1E1E, 0, 0], 0.1, 0.55, 0.0, 5, 0),
    skin("Carbon Fiber", Pattern::Carbon, [0x18191B, 0x3A3C40, 0, 0], 0.25, 0.3, 0.0, 5, 0),
    skin("Damascus", Pattern::Damascus, [0xB8BEC4, 0x50555A, 0, 0], 0.5, 0.3, 0.0, 6, 0),
    skin("Gold", Pattern::Solid, [0xE8B840, 0x2A2A2A, 0, 0], 0.5, 0.25, 0.12, 7, 0),
    skin("Neon Grid", Pattern::Grid, [0x101418, 0x00E5FF, 0, 0], 0.2, 0.35, 3.0, 8, 0),
    skin("Diamond", Pattern::Solid, [0xBFEFFF, 0x7FD4F0, 0, 0], 0.4, 0.08, 0.18, 0, 1),
    skin("Inferno", Pattern::Cracks, [0x1A0F0C, 0xFF5A1A, 0, 0], 0.15, 0.6, 4.0, 0, 2),
];

pub const RETICLES: [(&str, u32); 6] = [
    ("Red", 0xFF2A2A),
    ("Green", 0x2AFF5A),
    ("Cyan", 0x2AE8FF),
    ("Amber", 0xFFB030),
    ("Magenta", 0xFF3AE0),
    ("White", 0xFFFFFF),
];

/// One weapon's customization.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GunMods {
    pub optic: u8,
    pub muzzle: u8,
    pub under: u8,
    pub mag: u8,
    pub stock: u8,
    pub skin: u8,
    pub reticle: u8,
}

impl GunMods {
    pub fn factory(weapon: &str) -> Self {
        let mut m = Self::default();
        for s in Slot::ALL {
            m.set(s, options(weapon, s).first().copied().unwrap_or(0));
        }
        m
    }
    pub fn get(&self, s: Slot) -> u8 {
        match s {
            Slot::Optic => self.optic,
            Slot::Muzzle => self.muzzle,
            Slot::Under => self.under,
            Slot::Mag => self.mag,
            Slot::Stock => self.stock,
        }
    }
    pub fn set(&mut self, s: Slot, v: u8) {
        match s {
            Slot::Optic => self.optic = v,
            Slot::Muzzle => self.muzzle = v,
            Slot::Under => self.under = v,
            Slot::Mag => self.mag = v,
            Slot::Stock => self.stock = v,
        }
    }
    /// 21 bits, exact in an f32 for the network.
    pub fn pack(&self) -> u32 {
        (self.optic as u32 & 7)
            | (self.muzzle as u32 & 7) << 3
            | (self.under as u32 & 7) << 6
            | (self.mag as u32 & 3) << 9
            | (self.stock as u32 & 3) << 11
            | (self.skin as u32 & 31) << 13
            | (self.reticle as u32 & 7) << 18
    }
    pub fn unpack(v: u32) -> Self {
        Self {
            optic: (v & 7) as u8,
            muzzle: (v >> 3 & 7) as u8,
            under: (v >> 6 & 7) as u8,
            mag: (v >> 9 & 3) as u8,
            stock: (v >> 11 & 3) as u8,
            skin: (v >> 13 & 31) as u8,
            reticle: (v >> 18 & 7) as u8,
        }
    }
    pub fn sanitized(mut self, weapon: &str) -> Self {
        for s in Slot::ALL {
            let opts = options(weapon, s);
            if !opts.contains(&self.get(s)) {
                self.set(s, opts.first().copied().unwrap_or(0));
            }
        }
        if self.skin as usize >= SKINS.len() {
            self.skin = 0;
        }
        if self.reticle as usize >= RETICLES.len() {
            self.reticle = 0;
        }
        self
    }
}

pub fn attachment_unlocked(slot: Slot, idx: u8, rank: u32) -> bool {
    table(slot).get(idx as usize).is_some_and(|a| rank >= a.rank)
}

pub fn skin_unlocked(idx: u8, rank: u32, rebirths: u32) -> bool {
    SKINS.get(idx as usize).is_some_and(|s| rank >= s.rank && rebirths >= s.rebirths)
}

/// Feel of a weapon after attachments.
#[derive(Clone, Copy, Debug)]
pub struct Handling {
    /// Camera kick per shot, degrees.
    pub recoil: f32,
    pub ads_time: f32,
    pub zoom: f32,
    pub hip: f32,
    pub scoped: bool,
    pub suppressed: bool,
    pub flash: f32,
}

impl Default for Handling {
    fn default() -> Self {
        Self { recoil: 0.0, ads_time: 0.2, zoom: 1.0, hip: 1.0, scoped: false, suppressed: false, flash: 1.0 }
    }
}

/// Applies attachments to a weapon definition.
pub fn apply(def: &WeaponDef, m: &GunMods) -> (WeaponDef, Handling) {
    let id = def.id.as_str();
    let mut fx = M;
    for s in Slot::ALL {
        if options(id, s).is_empty() {
            continue;
        }
        if let Some(a) = table(s).get(m.get(s) as usize) {
            fx.spread *= a.fx.spread;
            fx.recoil *= a.fx.recoil;
            fx.ads *= a.fx.ads;
            fx.mag *= a.fx.mag;
            fx.reload *= a.fx.reload;
            fx.range *= a.fx.range;
            fx.hip *= a.fx.hip;
        }
    }
    let mut d = def.clone();
    if d.mag > 1 {
        d.mag = ((d.mag as f32) * fx.mag).round() as u32;
    }
    d.reload *= fx.reload;
    if let WeaponKind::Hitscan { range, spread, pellets } = d.kind {
        d.kind = WeaponKind::Hitscan { range: range * fx.range, spread: spread * fx.spread, pellets };
    }
    let (recoil, ads, flash) = match id {
        "pistol" => (1.6, 0.14, 0.7),
        "rifle" => (0.7, 0.22, 1.0),
        "smg" => (0.5, 0.17, 0.8),
        "shotgun" => (4.0, 0.22, 1.5),
        "sniper" => (5.0, 0.32, 1.6),
        "rocket" => (3.0, 0.35, 0.0),
        _ => (0.0, 0.2, 0.0),
    };
    let has_optics = !options(id, Slot::Optic).is_empty();
    let optic = if has_optics { m.optic as usize } else { 0 };
    let zoom = if matches!(id, "knife" | "grenade") { 1.0 } else { OPTIC_ZOOM[optic.min(4)] };
    let muzzle = if options(id, Slot::Muzzle).is_empty() { 0 } else { m.muzzle };
    let flash_mult = [1.0, 0.45, 0.9, 0.2, 1.4][muzzle.min(4) as usize];
    (
        d,
        Handling {
            recoil: recoil * fx.recoil,
            ads_time: ads * fx.ads,
            zoom,
            hip: fx.hip,
            scoped: optic >= 3,
            suppressed: muzzle == 3,
            flash: flash * flash_mult,
        },
    )
}

// ---------------------------------------------------------------- armory

/// Every weapon's customization, saved next to the settings.
#[derive(Resource, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Armory {
    pub mods: HashMap<String, GunMods>,
    #[serde(skip)]
    pub no_save: bool,
}

impl Armory {
    fn path() -> Option<std::path::PathBuf> {
        Some(config_dir()?.join("armory.json"))
    }
    pub fn load() -> Self {
        let mut a: Armory = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // Scripted runs never touch the player's saved loadouts.
        a.no_save = std::env::var("FF_AUTO").is_ok();
        if a.no_save {
            a.mods.clear();
        }
        a
    }
    pub fn get(&self, weapon: &str) -> GunMods {
        self.mods.get(weapon).copied().unwrap_or_else(|| GunMods::factory(weapon)).sanitized(weapon)
    }
    pub fn set(&mut self, weapon: &str, m: GunMods) {
        self.mods.insert(weapon.to_string(), m.sanitized(weapon));
        if self.no_save {
            return;
        }
        let Some(p) = Self::path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serde_json::to_string_pretty(self).unwrap_or_default());
    }
}

// ---------------------------------------------------------------- textures

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263) ^ seed.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535.0
}

/// Tileable value noise on [0,1)².
fn vn(u: f32, v: f32, cells: i32, seed: u32) -> f32 {
    let (fx, fy) = (u * cells as f32, v * cells as f32);
    let (ix, iy) = (fx.floor() as i32, fy.floor() as i32);
    let (tx, ty) = (fx - ix as f32, fy - iy as f32);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let h = |a: i32, b: i32| hash(a.rem_euclid(cells), b.rem_euclid(cells), seed);
    let a = h(ix, iy) + (h(ix + 1, iy) - h(ix, iy)) * s(tx);
    let b = h(ix, iy + 1) + (h(ix + 1, iy + 1) - h(ix, iy + 1)) * s(tx);
    a + (b - a) * s(ty)
}

fn fbm(u: f32, v: f32, cells: i32, seed: u32) -> f32 {
    0.6 * vn(u, v, cells, seed) + 0.3 * vn(u, v, cells * 2, seed + 1) + 0.1 * vn(u, v, cells * 4, seed + 2)
}

fn rgbf(c: u32) -> [f32; 3] {
    [((c >> 16) & 255) as f32, ((c >> 8) & 255) as f32, (c & 255) as f32]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

const TEX: usize = 128;
/// Metres covered by one texture tile.
const TILE: f32 = 0.16;

/// Base colour and optional glow pixels for a patterned finish.
fn pattern_pixels(s: &Skin) -> (Vec<u8>, Option<Vec<u8>>) {
    let c = s.colors.map(rgbf);
    let mut px = Vec::with_capacity(TEX * TEX * 4);
    let mut glow = Vec::with_capacity(TEX * TEX * 4);
    let mut any_glow = false;
    for y in 0..TEX {
        for x in 0..TEX {
            let (u, v) = (x as f32 / TEX as f32, y as f32 / TEX as f32);
            let mut g = [0.0f32; 3];
            let col = match s.pattern {
                Pattern::Blobs | Pattern::Digital => {
                    let (u, v) = if s.pattern == Pattern::Digital {
                        (((u * 32.0).floor() + 0.5) / 32.0, ((v * 32.0).floor() + 0.5) / 32.0)
                    } else {
                        (u, v)
                    };
                    let mut col = c[0];
                    if fbm(u, v, 3, 1) > 0.55 {
                        col = c[1];
                    }
                    if fbm(u, v, 4, 7) > 0.58 {
                        col = c[2];
                    }
                    if fbm(u, v, 5, 13) > 0.64 {
                        col = c[3];
                    }
                    col
                }
                Pattern::Tiger => {
                    let w = fbm(u, v, 3, 7);
                    let stripe = ((u * 3.0 + w * 1.3) * TAU).sin();
                    if stripe > 0.55 {
                        c[1]
                    } else if stripe < -0.8 {
                        c[2]
                    } else {
                        c[0]
                    }
                }
                Pattern::Carbon => {
                    let (cx, cy) = (x / 8, y / 8);
                    let t = if (cx + cy) % 2 == 0 { (x % 8) as f32 / 8.0 } else { (y % 8) as f32 / 8.0 };
                    mix(c[0], c[1], (t * PI).sin() * 0.85)
                }
                Pattern::Hazard => {
                    if ((x + y) / 16) % 2 == 0 {
                        c[0]
                    } else {
                        c[1]
                    }
                }
                Pattern::Damascus => {
                    let w = fbm(u, v, 3, 9);
                    let band = ((v * 6.0 + w * 2.2) * TAU).sin() * 0.5 + 0.5;
                    mix(c[1], c[0], band)
                }
                Pattern::Grid => {
                    if x % 32 == 0 || y % 32 == 0 {
                        g = c[1];
                        c[1]
                    } else {
                        mix(c[0], [40.0, 50.0, 60.0], vn(u, v, 8, 3) * 0.3)
                    }
                }
                Pattern::Cracks => {
                    let n = fbm(u, v, 4, 11);
                    let m = fbm(u, v, 5, 21);
                    if (n - 0.5).abs() < 0.022 || (m - 0.52).abs() < 0.016 {
                        g = c[1];
                        c[1]
                    } else {
                        mix(c[0], [60.0, 30.0, 20.0], vn(u, v, 16, 5) * 0.5)
                    }
                }
                Pattern::Factory | Pattern::Solid => c[0],
            };
            let grain = 0.95 + 0.1 * hash(x as i32, y as i32, 99);
            for k in 0..3 {
                px.push((col[k] * grain).clamp(0.0, 255.0) as u8);
                glow.push(g[k] as u8);
            }
            px.push(255);
            glow.push(255);
            any_glow |= g != [0.0; 3];
        }
    }
    (px, any_glow.then_some(glow))
}

fn make_image(px: Vec<u8>) -> Image {
    let mut img = Image::new(
        Extent3d { width: TEX as u32, height: TEX as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        px,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    img
}

/// Planar box-mapped UVs in metres, so patterns keep their scale on every part.
fn box_uv(mut m: Mesh) -> Mesh {
    let pos = match m.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(v)) => v.clone(),
        _ => return m,
    };
    let nor = match m.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(v)) => v.clone(),
        _ => return m,
    };
    let uv: Vec<[f32; 2]> = pos
        .iter()
        .zip(&nor)
        .map(|(p, n)| {
            let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
            let (u, v) = if ax >= ay && ax >= az {
                (p[2], p[1])
            } else if ay >= az {
                (p[0], p[2])
            } else {
                (p[0], p[1])
            };
            [u / TILE + 0.37, v / TILE + 0.61]
        })
        .collect();
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m
}

// ---------------------------------------------------------------- builder

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Body,
    Accent,
    Metal,
    Dark,
    Steel,
    Brass,
    Lens,
    Reticle,
    Laser,
    Flash,
    Hazard,
    Warhead,
    Glove,
    Sleeve,
    Cuff,
}

/// Parts that animate on a gun.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnimKind {
    Mag,
    Pump,
    Slide,
    Bolt,
    RocketTip,
    Flash,
}

#[derive(Component)]
pub struct GunAnim {
    pub kind: AnimKind,
    pub base: Transform,
    pub vm: bool,
}

/// Key points of a built gun, in gun-local space.
#[derive(Component, Clone, Copy, Default, Debug)]
pub struct GunInfo {
    pub muzzle: Vec3,
    pub sight: Vec3,
    pub ads_dist: f32,
    pub grip: Vec3,
    pub support: Option<Vec3>,
    pub eject: Option<Vec3>,
    pub zmin: f32,
    pub zmax: f32,
}

/// Parameters for one build.
pub struct Ctx {
    pub skin: u8,
    pub reticle: u8,
    pub factory: u8,
    pub vm: bool,
    pub team: u32,
}

#[derive(Resource, Default)]
pub struct GunCache {
    meshes: HashMap<(u8, [i32; 3]), Handle<Mesh>>,
    mats: HashMap<(Role, u32), Handle<StandardMaterial>>,
    tex: HashMap<u8, (Handle<Image>, Option<Handle<Image>>)>,
}

#[derive(SystemParam)]
pub struct GunKit<'w, 's> {
    pub commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    images: ResMut<'w, Assets<Image>>,
    cache: ResMut<'w, GunCache>,
}

/// Point in gun space from (right, up, forward).
fn p(x: f32, y: f32, f: f32) -> Vec3 {
    Vec3::new(x, y, -f)
}

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// Positive tilts the bottom of a part toward the back (pistol grips).
fn tilt(a: f32) -> Quat {
    Quat::from_rotation_x(-a)
}

fn q(x: f32) -> i32 {
    (x * 2000.0).round() as i32
}

/// Factory furniture colour per weapon.
const FACTORY: [(u32, f32); 6] = [(0x2B2D30, 0.6), (0x7A4A26, 0.7), (0x4F5638, 0.75), (0x5E6B3A, 0.8), (0x4A5530, 0.8), (0x8A8F94, 0.35)];

fn factory_index(weapon: &str) -> u8 {
    match weapon {
        "shotgun" => 1,
        "sniper" => 2,
        "rocket" => 3,
        "grenade" => 4,
        "knife" => 0,
        _ => 0,
    }
}

impl GunKit<'_, '_> {
    fn mesh(&mut self, kind: u8, dims: Vec3) -> Handle<Mesh> {
        let key = (kind, [q(dims.x), q(dims.y), q(dims.z)]);
        if let Some(h) = self.cache.meshes.get(&key) {
            return h.clone();
        }
        let m = match kind {
            0 => Mesh::from(Cuboid::new(dims.x, dims.y, dims.z)),
            1 => Cylinder::new(dims.x, dims.y).mesh().resolution(16).build(),
            2 => Sphere::new(dims.x).mesh().uv(14, 10),
            _ => Cone { radius: dims.x, height: dims.y }.mesh().resolution(14).build(),
        };
        let h = self.meshes.add(box_uv(m));
        self.cache.meshes.insert(key, h.clone());
        h
    }

    fn skin_tex(&mut self, idx: u8) -> (Handle<Image>, Option<Handle<Image>>) {
        if let Some(t) = self.cache.tex.get(&idx) {
            return t.clone();
        }
        let (px, glow) = pattern_pixels(&SKINS[idx as usize]);
        let base = self.images.add(make_image(px));
        let glow = glow.map(|g| self.images.add(make_image(g)));
        self.cache.tex.insert(idx, (base.clone(), glow.clone()));
        (base, glow)
    }

    pub fn mat(&mut self, role: Role, cx: &Ctx) -> Handle<StandardMaterial> {
        let param = match role {
            Role::Body | Role::Accent => (cx.skin as u32) << 8 | cx.factory as u32,
            Role::Reticle => cx.reticle as u32,
            Role::Cuff => cx.team,
            _ => 0,
        };
        if let Some(h) = self.cache.mats.get(&(role, param)) {
            return h.clone();
        }
        let col = |c: u32| crate::state::rgb(c);
        let plain = |c: u32, metallic: f32, rough: f32| StandardMaterial {
            base_color: col(c),
            metallic,
            perceptual_roughness: rough,
            reflectance: 0.5,
            ..default()
        };
        let glowing = |c: u32, k: f32| StandardMaterial {
            base_color: col(c),
            emissive: col(c).to_linear() * k,
            unlit: true,
            ..default()
        };
        let s = &SKINS[cx.skin as usize % SKINS.len()];
        let m = match role {
            Role::Body => match s.pattern {
                Pattern::Factory => {
                    let (c, r) = FACTORY[cx.factory as usize % FACTORY.len()];
                    plain(c, 0.1, r)
                }
                Pattern::Solid => StandardMaterial {
                    emissive: col(s.colors[0]).to_linear() * s.glow,
                    ..plain(s.colors[0], s.metallic, s.rough)
                },
                _ => {
                    let (tex, glow) = self.skin_tex(cx.skin);
                    StandardMaterial {
                        base_color: Color::WHITE,
                        base_color_texture: Some(tex),
                        metallic: s.metallic,
                        perceptual_roughness: s.rough,
                        emissive: if glow.is_some() { LinearRgba::WHITE * s.glow } else { LinearRgba::BLACK },
                        emissive_texture: glow,
                        ..default()
                    }
                }
            },
            Role::Accent => {
                if s.pattern == Pattern::Factory {
                    plain(0x3A3D40, 0.3, 0.5)
                } else if s.glow > 1.0 {
                    glowing(s.colors[1], s.glow * 0.6)
                } else {
                    plain(s.colors[1], s.metallic.max(0.2), s.rough)
                }
            }
            Role::Metal => plain(0x303236, 0.55, 0.38),
            Role::Dark => plain(0x1D1E20, 0.05, 0.65),
            Role::Steel => plain(0xB4B9BE, 0.65, 0.28),
            Role::Brass => plain(0xC9A043, 0.6, 0.3),
            Role::Lens => StandardMaterial {
                base_color: col(0x7FB8E0).with_alpha(0.22),
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 0.05,
                reflectance: 0.9,
                ..default()
            },
            Role::Reticle => glowing(RETICLES[cx.reticle as usize % RETICLES.len()].1, 6.0),
            Role::Laser => glowing(0xFF2020, 8.0),
            Role::Flash => StandardMaterial {
                base_color: col(0xFFC060),
                emissive: col(0xFFB040).to_linear() * 12.0,
                unlit: true,
                alpha_mode: AlphaMode::Add,
                ..default()
            },
            Role::Hazard => plain(0xE8A317, 0.1, 0.5),
            Role::Warhead => plain(0x4A5530, 0.2, 0.6),
            Role::Glove => plain(0x2E2B26, 0.0, 0.9),
            Role::Sleeve => plain(0x5E6B3A, 0.0, 0.95),
            Role::Cuff => plain(cx.team, 0.0, 0.8),
        };
        let h = self.materials.add(m);
        self.cache.mats.insert((role, param), h.clone());
        h
    }

    fn spawn_part(&mut self, cx: &Ctx, parent: Entity, mesh: Handle<Mesh>, role: Role, tr: Transform) -> Entity {
        let mat = self.mat(role, cx);
        let mut e = self.commands.spawn((Mesh3d(mesh), MeshMaterial3d(mat), tr));
        if cx.vm {
            e.insert((RenderLayers::layer(VM_LAYER), NotShadowCaster));
        }
        let id = e.id();
        self.commands.entity(parent).add_child(id);
        id
    }

    pub fn shell_mesh(&mut self, r: f32, len: f32) -> Handle<Mesh> {
        self.mesh(1, v(r, len, 0.0))
    }

    pub fn bx(&mut self, cx: &Ctx, parent: Entity, role: Role, c: Vec3, s: Vec3) -> Entity {
        self.bxr(cx, parent, role, c, s, Quat::IDENTITY)
    }

    pub fn bxr(&mut self, cx: &Ctx, parent: Entity, role: Role, c: Vec3, s: Vec3, rot: Quat) -> Entity {
        let mesh = self.mesh(0, s);
        self.spawn_part(cx, parent, mesh, role, Transform { translation: c, rotation: rot, ..default() })
    }

    fn cyl(&mut self, cx: &Ctx, parent: Entity, role: Role, c: Vec3, r: f32, len: f32, rot: Quat) -> Entity {
        let mesh = self.mesh(1, v(r, len, 0.0));
        self.spawn_part(cx, parent, mesh, role, Transform { translation: c, rotation: rot, ..default() })
    }

    /// Cylinder along the barrel axis.
    pub fn rod(&mut self, cx: &Ctx, parent: Entity, role: Role, c: Vec3, r: f32, len: f32) -> Entity {
        self.cyl(cx, parent, role, c, r, len, Quat::from_rotation_x(FRAC_PI_2))
    }

    pub fn rodx(&mut self, cx: &Ctx, parent: Entity, role: Role, c: Vec3, r: f32, len: f32) -> Entity {
        self.cyl(cx, parent, role, c, r, len, Quat::from_rotation_z(FRAC_PI_2))
    }

    pub fn rody(&mut self, cx: &Ctx, parent: Entity, role: Role, c: Vec3, r: f32, len: f32) -> Entity {
        self.cyl(cx, parent, role, c, r, len, Quat::IDENTITY)
    }

    pub fn ball(&mut self, cx: &Ctx, parent: Entity, role: Role, c: Vec3, r: f32) -> Entity {
        let mesh = self.mesh(2, v(r, 0.0, 0.0));
        self.spawn_part(cx, parent, mesh, role, Transform::from_translation(c))
    }

    /// Cone with its tip pointing forward.
    pub fn cone_fwd(&mut self, cx: &Ctx, parent: Entity, role: Role, c: Vec3, r: f32, h: f32) -> Entity {
        let mesh = self.mesh(3, v(r, h, 0.0));
        self.spawn_part(cx, parent, mesh, role, Transform { translation: c, rotation: Quat::from_rotation_x(-FRAC_PI_2), ..default() })
    }

    /// Hollow tube along the barrel axis (you can look through it).
    pub fn tube(&mut self, cx: &Ctx, parent: Entity, role: Role, c: Vec3, r: f32, len: f32, wall: f32) {
        let n = 12;
        let w = 2.0 * r * (PI / n as f32).tan() + 0.0008;
        for k in 0..n {
            let a = k as f32 * TAU / n as f32;
            let off = v(a.cos() * r, a.sin() * r, 0.0);
            self.bxr(cx, parent, role, c + off, v(w, wall, len), Quat::from_rotation_z(a + FRAC_PI_2));
        }
    }

    /// Box stretched between two points.
    pub fn seg(&mut self, cx: &Ctx, parent: Entity, role: Role, a: Vec3, b: Vec3, w: f32, h: f32) -> Entity {
        let d = b - a;
        let rot = Quat::from_rotation_arc(Vec3::Z, d.normalize_or(Vec3::Z));
        self.bxr(cx, parent, role, (a + b) / 2.0, v(w, h, d.length()), rot)
    }

    pub fn node(&mut self, parent: Entity, tr: Transform, anim: Option<AnimKind>, vm: bool) -> Entity {
        let mut e = self.commands.spawn((tr, Visibility::default()));
        if let Some(kind) = anim {
            e.insert(GunAnim { kind, base: tr, vm });
        }
        let id = e.id();
        self.commands.entity(parent).add_child(id);
        id
    }

    // ------------------------------------------------------------ attachments

    /// Mounts an optic on a rail whose top centre is `b`. Returns (sight point, eye distance).
    fn optic(&mut self, cx: &Ctx, r: Entity, idx: u8, b: Vec3) -> (Vec3, f32) {
        use Role::*;
        match idx {
            1 => {
                self.bx(cx, r, Metal, b + p(0.0, 0.007, 0.0), v(0.03, 0.014, 0.045));
                for s in [-1.0, 1.0] {
                    self.bx(cx, r, Metal, b + p(0.0165 * s, 0.03, 0.012), v(0.005, 0.034, 0.014));
                }
                self.bx(cx, r, Metal, b + p(0.0, 0.049, 0.012), v(0.038, 0.006, 0.016));
                self.bx(cx, r, Accent, b + p(0.021, 0.02, -0.006), v(0.008, 0.01, 0.012));
                self.bx(cx, r, Lens, b + p(0.0, 0.03, 0.017), v(0.028, 0.03, 0.002));
                self.ball(cx, r, Reticle, b + p(0.0, 0.03, 0.002), 0.0013);
                (b + p(0.0, 0.03, -0.01), 0.2)
            }
            2 => {
                self.bx(cx, r, Metal, b + p(0.0, 0.007, 0.0), v(0.04, 0.014, 0.08));
                self.bx(cx, r, Dark, b + p(0.0, 0.02, -0.022), v(0.044, 0.014, 0.034));
                self.bx(cx, r, Accent, b + p(0.023, 0.02, -0.022), v(0.004, 0.008, 0.02));
                for s in [-1.0, 1.0] {
                    self.bx(cx, r, Metal, b + p(0.022 * s, 0.042, 0.02), v(0.006, 0.05, 0.032));
                }
                self.bx(cx, r, Metal, b + p(0.0, 0.069, 0.02), v(0.05, 0.007, 0.036));
                self.bx(cx, r, Lens, b + p(0.0, 0.042, 0.032), v(0.038, 0.044, 0.002));
                let c = b + p(0.0, 0.042, 0.03);
                for k in 0..14 {
                    let a = k as f32 * TAU / 14.0;
                    self.bx(cx, r, Reticle, c + v(a.cos() * 0.0065, a.sin() * 0.0065, 0.0), v(0.0016, 0.0016, 0.0004));
                }
                self.ball(cx, r, Reticle, c, 0.0009);
                (b + p(0.0, 0.042, -0.02), 0.2)
            }
            3 | 4 => {
                let (len, rad, obj, h) = if idx == 3 { (0.17, 0.016, 0.022, 0.034) } else { (0.27, 0.018, 0.028, 0.04) };
                for f in [-len * 0.28, len * 0.28] {
                    self.bx(cx, r, Metal, b + p(0.0, 0.01, f), v(0.03, 0.02, 0.016));
                    self.tube(cx, r, Metal, b + p(0.0, h, f), rad + 0.004, 0.016, 0.004);
                    self.bx(cx, r, Steel, b + p(0.017, h - 0.012, f), v(0.006, 0.006, 0.01));
                }
                self.tube(cx, r, Dark, b + p(0.0, h, 0.0), rad, len, 0.003);
                self.tube(cx, r, Dark, b + p(0.0, h, len / 2.0 + 0.025), obj, 0.05, 0.003);
                self.tube(cx, r, Dark, b + p(0.0, h, len / 2.0 + 0.002), (rad + obj) / 2.0, 0.01, 0.003);
                self.tube(cx, r, Accent, b + p(0.0, h, len / 2.0 + 0.045), obj + 0.001, 0.01, 0.003);
                self.tube(cx, r, Dark, b + p(0.0, h, -(len / 2.0 + 0.02)), rad + 0.004, 0.04, 0.003);
                self.tube(cx, r, Dark, b + p(0.0, h, -(len / 2.0 + 0.045)), rad + 0.006, 0.012, 0.004);
                self.rody(cx, r, Metal, b + p(0.0, h + rad + 0.008, 0.0), 0.008, 0.016);
                self.rody(cx, r, Accent, b + p(0.0, h + rad + 0.017, 0.0), 0.0085, 0.003);
                self.rodx(cx, r, Metal, b + p(rad + 0.008, h, 0.0), 0.008, 0.016);
                self.rodx(cx, r, Metal, b + p(-(rad + 0.006), h, 0.0), 0.006, 0.012);
                self.rod(cx, r, Lens, b + p(0.0, h, len / 2.0 + 0.048), obj - 0.002, 0.002);
                self.rod(cx, r, Lens, b + p(0.0, h, -(len / 2.0 + 0.04)), rad + 0.002, 0.002);
                (b + p(0.0, h, -(len / 2.0 + 0.06)), 0.08)
            }
            _ => (b, 0.12),
        }
    }

    /// Fits a muzzle device at the barrel end `at`; returns the muzzle point.
    fn muzzle_dev(&mut self, cx: &Ctx, r: Entity, idx: u8, at: Vec3, rad: f32, birdcage: bool) -> Vec3 {
        use Role::*;
        match idx {
            1 => {
                self.rod(cx, r, Metal, at + p(0.0, 0.0, 0.035), rad + 0.005, 0.07);
                for (x, y) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                    let e = rad + 0.0052;
                    self.bx(cx, r, Dark, at + p(x * e, y * e, 0.05), v(if x != 0.0 { 0.002 } else { 0.006 }, if y != 0.0 { 0.002 } else { 0.006 }, 0.03));
                }
                at + p(0.0, 0.0, 0.07)
            }
            2 => {
                self.bx(cx, r, Metal, at + p(0.0, 0.0, 0.03), v(0.032, 0.03, 0.06));
                for i in 0..3 {
                    self.bx(cx, r, Dark, at + p(0.0, 0.0155, 0.012 + i as f32 * 0.017), v(0.018, 0.002, 0.008));
                    for s in [-1.0, 1.0] {
                        self.bx(cx, r, Dark, at + p(0.0165 * s, 0.006, 0.012 + i as f32 * 0.017), v(0.002, 0.008, 0.008));
                    }
                }
                at + p(0.0, 0.0, 0.06)
            }
            3 => {
                let sr = (rad + 0.012).max(0.019);
                self.rod(cx, r, Dark, at + p(0.0, 0.0, 0.11), sr, 0.2);
                self.rod(cx, r, Metal, at + p(0.0, 0.0, 0.012), sr + 0.001, 0.014);
                self.rod(cx, r, Metal, at + p(0.0, 0.0, 0.207), sr + 0.001, 0.014);
                self.rod(cx, r, Accent, at + p(0.0, 0.0, 0.11), sr + 0.0008, 0.03);
                at + p(0.0, 0.0, 0.215)
            }
            4 => {
                self.bx(cx, r, Metal, at + p(0.0, 0.0, 0.04), v(0.05, 0.03, 0.08));
                for i in 0..3 {
                    self.bx(cx, r, Dark, at + p(0.0, 0.0, 0.016 + i as f32 * 0.024), v(0.052, 0.022, 0.009));
                }
                self.bx(cx, r, Steel, at + p(0.0, 0.0, 0.081), v(0.03, 0.02, 0.003));
                at + p(0.0, 0.0, 0.08)
            }
            _ => {
                if birdcage {
                    self.rod(cx, r, Metal, at + p(0.0, 0.0, 0.025), rad + 0.004, 0.05);
                    for s in [-1.0, 1.0] {
                        self.bx(cx, r, Dark, at + p((rad + 0.004) * s, 0.0, 0.03), v(0.002, 0.005, 0.03));
                    }
                    self.bx(cx, r, Dark, at + p(0.0, rad + 0.004, 0.03), v(0.005, 0.002, 0.03));
                    at + p(0.0, 0.0, 0.05)
                } else {
                    at
                }
            }
        }
    }

    /// Underbarrel attachment at the handguard's bottom centre; returns the support-hand point.
    fn under(&mut self, cx: &Ctx, r: Entity, idx: u8, at: Vec3, centered_laser: bool) -> Option<Vec3> {
        use Role::*;
        match idx {
            1 => {
                self.bx(cx, r, Metal, at + p(0.0, -0.006, 0.0), v(0.026, 0.012, 0.045));
                self.rody(cx, r, Dark, at + p(0.0, -0.055, 0.0), 0.016, 0.09);
                for i in 0..4 {
                    self.rody(cx, r, Accent, at + p(0.0, -0.03 - i as f32 * 0.018, 0.0), 0.0165, 0.004);
                }
                self.rody(cx, r, Dark, at + p(0.0, -0.101, 0.0), 0.019, 0.008);
                Some(at + p(0.0, -0.06, 0.0))
            }
            2 => {
                self.bx(cx, r, Metal, at + p(0.0, -0.005, 0.0), v(0.026, 0.01, 0.07));
                self.bxr(cx, r, Dark, at + p(0.0, -0.022, -0.005), v(0.03, 0.03, 0.09), Quat::from_rotation_x(0.38));
                self.bxr(cx, r, Accent, at + p(0.0, -0.036, -0.01), v(0.032, 0.004, 0.05), Quat::from_rotation_x(0.38));
                Some(at + p(0.0, -0.03, -0.015))
            }
            3 => {
                let side = if centered_laser { 0.0 } else { 0.034 };
                let y = if centered_laser { -0.012 } else { 0.022 };
                self.bx(cx, r, Dark, at + p(side, y, 0.0), v(0.024, 0.024, 0.055));
                self.bx(cx, r, Accent, at + p(side, y + 0.0125, -0.01), v(0.012, 0.002, 0.02));
                self.ball(cx, r, Laser, at + p(side - 0.005, y, 0.028), 0.0045);
                self.ball(cx, r, Lens, at + p(side + 0.006, y, 0.0285), 0.0045);
                None
            }
            _ => None,
        }
    }

    fn flash(&mut self, cx: &Ctx, r: Entity, at: Vec3) {
        if !cx.vm {
            return;
        }
        let tr = Transform::from_translation(at);
        let f = self.node(r, tr, Some(AnimKind::Flash), true);
        self.commands.entity(f).insert(Visibility::Hidden);
        self.cone_fwd(cx, f, Role::Flash, p(0.0, 0.0, 0.05), 0.028, 0.1);
        for k in 0..4 {
            let rot = Quat::from_rotation_z(k as f32 * PI / 4.0);
            self.bxr(cx, f, Role::Flash, p(0.0, 0.0, 0.012), v(0.13 - k as f32 * 0.02, 0.012, 0.003), rot);
        }
        self.ball(cx, f, Role::Flash, p(0.0, 0.0, 0.01), 0.018);
    }

    // ------------------------------------------------------------ weapons

    /// Builds `weapon` under `root`. Returns the gun's key points.
    pub fn build_gun(&mut self, root: Entity, weapon: &str, m: GunMods, vm: bool, team: u32) -> GunInfo {
        let cx = Ctx { skin: m.skin, reticle: m.reticle, factory: factory_index(weapon), vm, team };
        let mut info = match weapon {
            "rifle" => self.rifle(&cx, root, &m),
            "smg" => self.smg(&cx, root, &m),
            "shotgun" => self.shotgun(&cx, root, &m),
            "sniper" => self.sniper(&cx, root, &m),
            "pistol" => self.pistol(&cx, root, &m),
            "rocket" => self.rocket(&cx, root, &m),
            "grenade" => self.grenade(&cx, root),
            _ => self.knife(&cx, root),
        };
        info.zmin = info.zmin.min(info.muzzle.z);
        if vm {
            self.arms(&cx, root, &info, weapon);
        }
        self.commands.entity(root).insert(info);
        info
    }

    fn rail(&mut self, cx: &Ctx, r: Entity, top: Vec3, len: f32, width: f32) {
        self.bx(cx, r, Role::Metal, top + p(0.0, -0.006, 0.0), v(width, 0.012, len));
        let n = (len / 0.03) as i32;
        for i in 0..n {
            let f = -len / 2.0 + 0.015 + i as f32 * 0.03;
            self.bx(cx, r, Role::Dark, top + p(0.0, 0.002, f), v(width + 0.004, 0.005, 0.012));
        }
    }

    fn iron_rear(&mut self, cx: &Ctx, r: Entity, at: Vec3) {
        self.bx(cx, r, Role::Metal, at + p(0.0, -0.012, 0.0), v(0.028, 0.012, 0.03));
        for s in [-1.0, 1.0] {
            self.bx(cx, r, Role::Metal, at + p(0.0085 * s, 0.002, 0.0), v(0.006, 0.022, 0.02));
        }
        self.bx(cx, r, Role::Steel, at + p(0.0155, -0.012, 0.0), v(0.003, 0.008, 0.008));
    }

    fn iron_front(&mut self, cx: &Ctx, r: Entity, top: Vec3) {
        self.bx(cx, r, Role::Metal, top + p(0.0, -0.026, 0.0), v(0.026, 0.012, 0.02));
        self.bx(cx, r, Role::Metal, top + p(0.0, -0.011, 0.0), v(0.0035, 0.022, 0.004));
        for s in [-1.0, 1.0] {
            self.bx(cx, r, Role::Metal, top + p(0.011 * s, -0.012, 0.0), v(0.004, 0.024, 0.012));
        }
    }

    fn stock_ar(&mut self, cx: &Ctx, r: Entity, idx: u8, y: f32) {
        use Role::*;
        self.rod(cx, r, Metal, p(0.0, y, -0.2), 0.014, 0.17);
        match idx {
            1 => {
                self.bx(cx, r, Metal, p(0.0, y + 0.002, -0.3), v(0.014, 0.014, 0.18));
                self.seg(cx, r, Metal, p(0.0, y - 0.01, -0.21), p(0.0, y - 0.072, -0.385), 0.012, 0.012);
                self.bx(cx, r, Dark, p(0.0, y - 0.03, -0.392), v(0.042, 0.1, 0.014));
                self.bx(cx, r, Accent, p(0.0, y - 0.03, -0.4), v(0.038, 0.08, 0.004));
            }
            2 => {
                self.bx(cx, r, Body, p(0.0, y - 0.022, -0.31), v(0.054, 0.11, 0.17));
                self.bx(cx, r, Accent, p(0.0, y + 0.042, -0.3), v(0.044, 0.026, 0.13));
                self.bx(cx, r, Dark, p(0.0, y - 0.025, -0.402), v(0.056, 0.13, 0.018));
                for s in [-1.0, 1.0] {
                    self.bx(cx, r, Dark, p(0.0275 * s, y - 0.03, -0.31), v(0.002, 0.05, 0.1));
                }
            }
            3 => {
                self.bx(cx, r, Body, p(0.0, y - 0.012, -0.31), v(0.046, 0.07, 0.17));
                self.bx(cx, r, Body, p(0.0, y - 0.07, -0.33), v(0.04, 0.045, 0.12));
                self.bx(cx, r, Accent, p(0.0, y + 0.04, -0.3), v(0.04, 0.016, 0.11));
                for f in [-0.27, -0.33] {
                    self.rody(cx, r, Steel, p(0.0, y + 0.026, f), 0.004, 0.02);
                }
                self.bx(cx, r, Dark, p(0.0, y - 0.03, -0.402), v(0.048, 0.13, 0.016));
                self.rodx(cx, r, Metal, p(0.0, y - 0.03, -0.25), 0.008, 0.05);
            }
            _ => {
                self.bx(cx, r, Body, p(0.0, y - 0.018, -0.31), v(0.044, 0.082, 0.15));
                self.bxr(cx, r, Body, p(0.0, y - 0.062, -0.265), v(0.04, 0.03, 0.09), tilt(-0.3));
                self.bx(cx, r, Dark, p(0.0, y - 0.025, -0.392), v(0.046, 0.11, 0.016));
                for s in [-1.0, 1.0] {
                    self.bx(cx, r, Accent, p(0.0225 * s, y - 0.01, -0.3), v(0.002, 0.03, 0.08));
                }
            }
        }
    }

    fn mag_box(&mut self, cx: &Ctx, n: Entity, idx: u8, w: f32, d: f32, h: f32, curve: f32) {
        use Role::*;
        match idx {
            1 => {
                self.bx(cx, n, Dark, p(0.0, -0.06, 0.0), v(w, 0.13, d));
                self.bxr(cx, n, Dark, p(0.0, -0.06 - h * 0.6, 0.032 * curve), v(w, h * 0.9, d), tilt(-0.32 * curve));
                self.bxr(cx, n, Accent, p(0.0, -0.065 - h * 1.05, 0.05 * curve), v(w + 0.004, 0.012, d + 0.006), tilt(-0.32 * curve));
            }
            2 => {
                for s in [0.0, 1.0] {
                    let x = s * (w + 0.004);
                    self.bx(cx, n, Dark, p(x, -0.045, 0.0), v(w, 0.09, d));
                    self.bxr(cx, n, Dark, p(x, -0.045 - h * 0.45, 0.018 * curve), v(w, h * 0.7, d), tilt(-0.28 * curve));
                }
                self.bx(cx, n, Accent, p((w + 0.004) / 2.0, -0.07, 0.0), v(w * 2.0 + 0.012, 0.014, d + 0.008));
                self.bx(cx, n, Accent, p((w + 0.004) / 2.0, -0.02, 0.0), v(w * 2.0 + 0.012, 0.01, d + 0.008));
            }
            3 => {
                self.bx(cx, n, Dark, p(0.0, -0.03, 0.0), v(w, 0.06, d));
                self.rodx(cx, n, Dark, p(0.0, -0.1, 0.0), 0.065, w * 2.0);
                self.rodx(cx, n, Accent, p(0.0, -0.1, 0.0), 0.03, w * 2.0 + 0.003);
                self.rodx(cx, n, Metal, p(0.0, -0.1, 0.0), 0.012, w * 2.0 + 0.006);
            }
            _ => {
                self.bx(cx, n, Dark, p(0.0, -0.045, 0.0), v(w, 0.09, d));
                self.bxr(cx, n, Dark, p(0.0, -0.045 - h * 0.45, 0.018 * curve), v(w, h * 0.7, d), tilt(-0.28 * curve));
                self.bxr(cx, n, Accent, p(0.0, -0.05 - h * 0.8, 0.03 * curve), v(w + 0.004, 0.01, d + 0.006), tilt(-0.28 * curve));
            }
        }
    }

    fn rifle(&mut self, cx: &Ctx, r: Entity, m: &GunMods) -> GunInfo {
        use Role::*;
        // Receivers.
        self.bx(cx, r, Body, p(0.0, -0.012, 0.0), v(0.046, 0.05, 0.22));
        self.bx(cx, r, Body, p(0.0, 0.035, 0.03), v(0.05, 0.052, 0.3));
        self.bx(cx, r, Accent, p(0.0262, 0.036, 0.05), v(0.003, 0.022, 0.07));
        self.bx(cx, r, Dark, p(0.0255, 0.036, 0.012), v(0.002, 0.016, 0.006));
        self.rod(cx, r, Metal, p(0.03, 0.045, -0.06), 0.008, 0.03);
        self.bx(cx, r, Metal, p(0.0, 0.05, -0.13), v(0.035, 0.012, 0.02));
        self.bx(cx, r, Metal, p(0.0, 0.05, -0.142), v(0.06, 0.012, 0.01));
        self.bx(cx, r, Steel, p(-0.026, 0.0, -0.02), v(0.004, 0.012, 0.02));
        self.rodx(cx, r, Steel, p(0.0, -0.005, 0.07), 0.004, 0.052);
        // Top rail and handguard.
        self.rail(cx, r, p(0.0, 0.074, 0.18), 0.6, 0.028);
        self.bx(cx, r, Body, p(0.0, 0.032, 0.33), v(0.058, 0.062, 0.3));
        for i in 0..5 {
            for s in [-1.0, 1.0] {
                self.bx(cx, r, Dark, p(0.0295 * s, 0.03, 0.22 + i as f32 * 0.055), v(0.003, 0.022, 0.032));
            }
        }
        self.bx(cx, r, Metal, p(0.0, -0.002, 0.37), v(0.024, 0.008, 0.2));
        self.rod(cx, r, Accent, p(0.0, 0.032, 0.185), 0.034, 0.012);
        // Barrel and gas block.
        self.rod(cx, r, Metal, p(0.0, 0.032, 0.58), 0.011, 0.2);
        self.bx(cx, r, Metal, p(0.0, 0.04, 0.5), v(0.024, 0.032, 0.026));
        // Grip, trigger, magwell.
        self.bxr(cx, r, Dark, p(0.0, -0.075, -0.035), v(0.034, 0.095, 0.045), tilt(0.32));
        self.bxr(cx, r, Dark, p(0.0, -0.118, -0.05), v(0.036, 0.01, 0.05), tilt(0.32));
        self.bx(cx, r, Metal, p(0.0, -0.046, 0.025), v(0.008, 0.006, 0.065));
        self.bx(cx, r, Metal, p(0.0, -0.04, 0.056), v(0.008, 0.018, 0.006));
        self.bxr(cx, r, Steel, p(0.0, -0.032, 0.03), v(0.005, 0.02, 0.006), tilt(0.25));
        self.bx(cx, r, Body, p(0.0, -0.045, 0.085), v(0.05, 0.03, 0.07));
        let mag = self.node(r, Transform::from_translation(p(0.0, -0.06, 0.088)), Some(AnimKind::Mag), cx.vm);
        self.mag_box(cx, mag, m.mag, 0.028, 0.055, 0.11, 1.0);
        self.stock_ar(cx, r, m.stock, 0.03);
        // Sights.
        let (sight, ads) = if m.optic == 0 {
            self.iron_front(cx, r, p(0.0, 0.104, 0.47));
            self.iron_rear(cx, r, p(0.0, 0.094, -0.08));
            (p(0.0, 0.104, -0.08), 0.14)
        } else {
            self.optic(cx, r, m.optic, p(0.0, 0.074, 0.02))
        };
        let muzzle = self.muzzle_dev(cx, r, m.muzzle, p(0.0, 0.032, 0.68), 0.011, true);
        self.flash(cx, r, muzzle);
        let support = self.under(cx, r, m.under, p(0.0, -0.006, 0.38), false).or(Some(p(0.0, -0.012, 0.38)));
        GunInfo { muzzle, sight, ads_dist: ads, grip: p(0.0, -0.05, -0.025), support, eject: Some(p(0.03, 0.036, 0.05)), zmin: 0.0, zmax: 0.41 }
    }

    fn smg(&mut self, cx: &Ctx, r: Entity, m: &GunMods) -> GunInfo {
        use Role::*;
        self.bx(cx, r, Body, p(0.0, 0.025, 0.03), v(0.05, 0.072, 0.28));
        for s in [-1.0, 1.0] {
            self.bx(cx, r, Accent, p(0.0255 * s, 0.048, 0.03), v(0.002, 0.012, 0.26));
            self.bx(cx, r, Dark, p(0.0255 * s, 0.01, -0.06), v(0.002, 0.02, 0.05));
        }
        self.rail(cx, r, p(0.0, 0.073, 0.03), 0.24, 0.026);
        self.bx(cx, r, Dark, p(0.0, 0.022, 0.22), v(0.046, 0.052, 0.1));
        for i in 0..3 {
            for s in [-1.0, 1.0] {
                self.bx(cx, r, Metal, p(0.0235 * s, 0.024, 0.19 + i as f32 * 0.025), v(0.002, 0.026, 0.012));
            }
        }
        self.rod(cx, r, Metal, p(0.0, 0.025, 0.3), 0.01, 0.08);
        self.bxr(cx, r, Body, p(0.0, -0.028, 0.205), v(0.03, 0.05, 0.035), tilt(-0.25));
        self.bxr(cx, r, Dark, p(0.0, -0.065, -0.03), v(0.034, 0.09, 0.044), tilt(0.25));
        self.bx(cx, r, Metal, p(0.0, -0.032, 0.02), v(0.008, 0.006, 0.06));
        self.bxr(cx, r, Steel, p(0.0, -0.02, 0.025), v(0.005, 0.018, 0.006), tilt(0.25));
        self.bx(cx, r, Metal, p(-0.03, 0.04, 0.12), v(0.014, 0.012, 0.025));
        self.bx(cx, r, Body, p(0.0, -0.02, 0.09), v(0.036, 0.024, 0.05));
        let mag = self.node(r, Transform::from_translation(p(0.0, -0.025, 0.09)), Some(AnimKind::Mag), cx.vm);
        self.mag_box(cx, mag, m.mag, 0.024, 0.04, 0.08, 0.3);
        match m.stock {
            1 => {
                self.bx(cx, r, Dark, p(0.0, 0.02, -0.125), v(0.034, 0.05, 0.03));
                self.bx(cx, r, Accent, p(0.0, 0.02, -0.142), v(0.03, 0.04, 0.004));
            }
            2 | 3 => self.stock_ar(cx, r, m.stock, 0.03),
            _ => {
                self.bx(cx, r, Metal, p(0.0, 0.02, -0.12), v(0.04, 0.05, 0.02));
                for s in [-1.0, 1.0] {
                    self.bx(cx, r, Metal, p(0.016 * s, 0.045, -0.23), v(0.007, 0.007, 0.22));
                    self.bx(cx, r, Metal, p(0.016 * s, -0.005, -0.23), v(0.007, 0.007, 0.22));
                }
                self.bx(cx, r, Dark, p(0.0, 0.02, -0.345), v(0.044, 0.09, 0.012));
            }
        }
        let (sight, ads) = if m.optic == 0 {
            self.iron_front(cx, r, p(0.0, 0.1, 0.15));
            self.iron_rear(cx, r, p(0.0, 0.09, -0.09));
            (p(0.0, 0.1, -0.09), 0.14)
        } else {
            self.optic(cx, r, m.optic, p(0.0, 0.073, 0.01))
        };
        let muzzle = self.muzzle_dev(cx, r, m.muzzle, p(0.0, 0.025, 0.34), 0.01, true);
        self.flash(cx, r, muzzle);
        let support = self.under(cx, r, m.under, p(0.0, -0.003, 0.25), false).or(Some(p(0.0, -0.04, 0.205)));
        GunInfo { muzzle, sight, ads_dist: ads, grip: p(0.0, -0.05, -0.02), support, eject: Some(p(0.027, 0.04, 0.06)), zmin: 0.0, zmax: 0.35 }
    }

    fn shotgun(&mut self, cx: &Ctx, r: Entity, m: &GunMods) -> GunInfo {
        use Role::*;
        self.bx(cx, r, Metal, p(0.0, 0.025, 0.02), v(0.05, 0.068, 0.2));
        self.bx(cx, r, Dark, p(0.0255, 0.035, 0.04), v(0.002, 0.022, 0.06));
        self.bx(cx, r, Accent, p(-0.0255, 0.025, 0.02), v(0.002, 0.04, 0.14));
        self.bx(cx, r, Accent, p(0.0255, 0.012, -0.03), v(0.002, 0.02, 0.06));
        self.rod(cx, r, Metal, p(0.0, 0.042, 0.42), 0.012, 0.6);
        self.bx(cx, r, Metal, p(0.0, 0.057, 0.42), v(0.008, 0.004, 0.6));
        let tube_len = if m.mag == 1 { 0.58 } else { 0.46 };
        let tube_c = 0.12 + tube_len / 2.0;
        self.rod(cx, r, Metal, p(0.0, 0.006, tube_c), 0.011, tube_len);
        self.rod(cx, r, Steel, p(0.0, 0.006, 0.12 + tube_len + 0.006), 0.012, 0.012);
        self.bx(cx, r, Metal, p(0.0, 0.024, 0.12 + tube_len - 0.02), v(0.016, 0.05, 0.02));
        let pump = self.node(r, Transform::from_translation(p(0.0, 0.006, 0.3)), Some(AnimKind::Pump), cx.vm);
        self.bx(cx, pump, Body, Vec3::ZERO, v(0.048, 0.046, 0.16));
        for i in 0..6 {
            self.bx(cx, pump, Dark, p(0.0, -0.0235, -0.065 + i as f32 * 0.026), v(0.04, 0.002, 0.008));
            for s in [-1.0, 1.0] {
                self.bx(cx, pump, Dark, p(0.0245 * s, -0.004, -0.065 + i as f32 * 0.026), v(0.002, 0.03, 0.008));
            }
        }
        // Stock.
        match m.stock {
            1 => {
                self.bxr(cx, r, Dark, p(0.0, -0.055, -0.04), v(0.034, 0.09, 0.044), tilt(0.3));
                self.rod(cx, r, Metal, p(0.0, 0.02, -0.12), 0.012, 0.06);
                self.bx(cx, r, Dark, p(0.0, 0.02, -0.155), v(0.04, 0.05, 0.012));
            }
            2 => {
                self.bxr(cx, r, Dark, p(0.0, -0.055, -0.04), v(0.034, 0.09, 0.044), tilt(0.3));
                self.stock_ar(cx, r, 2, 0.022);
            }
            _ => {
                self.bxr(cx, r, Body, p(0.0, -0.02, -0.09), v(0.04, 0.06, 0.1), tilt(0.55));
                self.bxr(cx, r, Body, p(0.0, -0.022, -0.25), v(0.044, 0.085, 0.26), tilt(0.1));
                self.bxr(cx, r, Dark, p(0.0, -0.04, -0.384), v(0.048, 0.115, 0.018), tilt(0.1));
            }
        }
        self.bx(cx, r, Metal, p(0.0, -0.035, 0.03), v(0.008, 0.006, 0.06));
        self.bx(cx, r, Metal, p(0.0, -0.022, 0.06), v(0.008, 0.026, 0.006));
        self.bxr(cx, r, Steel, p(0.0, -0.02, 0.03), v(0.005, 0.02, 0.006), tilt(0.25));
        let (sight, ads) = if m.optic == 0 {
            self.ball(cx, r, Steel, p(0.0, 0.064, 0.71), 0.004);
            self.bx(cx, r, Metal, p(0.0, 0.063, -0.05), v(0.02, 0.01, 0.012));
            (p(0.0, 0.066, -0.05), 0.14)
        } else {
            self.optic(cx, r, m.optic, p(0.0, 0.059, 0.0))
        };
        let muzzle = self.muzzle_dev(cx, r, m.muzzle, p(0.0, 0.042, 0.72), 0.012, false);
        self.flash(cx, r, muzzle);
        let local = self.under(cx, pump, m.under, p(0.0, -0.023, 0.0), false);
        let support = Some(p(0.0, 0.006, 0.3) + local.unwrap_or(p(0.0, -0.016, 0.0)));
        GunInfo { muzzle, sight, ads_dist: ads, grip: p(0.0, -0.05, -0.05), support, eject: Some(p(0.028, 0.035, 0.04)), zmin: 0.0, zmax: 0.4 }
    }

    fn sniper(&mut self, cx: &Ctx, r: Entity, m: &GunMods) -> GunInfo {
        use Role::*;
        self.bx(cx, r, Body, p(0.0, 0.0, 0.05), v(0.056, 0.05, 0.42));
        self.bx(cx, r, Body, p(0.0, 0.01, 0.4), v(0.05, 0.044, 0.28));
        for i in 0..4 {
            for s in [-1.0, 1.0] {
                self.bx(cx, r, Dark, p(0.0255 * s, 0.01, 0.32 + i as f32 * 0.05), v(0.002, 0.018, 0.03));
            }
        }
        self.rod(cx, r, Metal, p(0.0, 0.04, 0.02), 0.02, 0.26);
        self.bx(cx, r, Dark, p(0.0195, 0.044, 0.04), v(0.002, 0.014, 0.06));
        self.rail(cx, r, p(0.0, 0.066, 0.02), 0.24, 0.026);
        let bolt = self.node(r, Transform::from_translation(p(0.02, 0.045, -0.07)), Some(AnimKind::Bolt), cx.vm);
        self.rodx(cx, bolt, Steel, p(0.022, 0.0, 0.0), 0.005, 0.045);
        self.ball(cx, bolt, Dark, p(0.046, -0.004, 0.0), 0.011);
        self.rod(cx, r, Metal, p(0.0, 0.04, 0.55), 0.013, 0.8);
        for (x, y) in [(0.0128, 0.0), (-0.0128, 0.0), (0.0, 0.0128)] {
            self.bx(cx, r, Dark, p(x, 0.04 + y, 0.72), v(if x != 0.0 { 0.002 } else { 0.004 }, if y != 0.0 { 0.002 } else { 0.004 }, 0.32));
        }
        self.bxr(cx, r, Body, p(0.0, -0.06, -0.08), v(0.038, 0.085, 0.048), tilt(0.35));
        self.bx(cx, r, Metal, p(0.0, -0.035, -0.02), v(0.008, 0.006, 0.06));
        self.bxr(cx, r, Steel, p(0.0, -0.02, -0.02), v(0.005, 0.02, 0.006), tilt(0.25));
        let mag = self.node(r, Transform::from_translation(p(0.0, -0.025, 0.06)), Some(AnimKind::Mag), cx.vm);
        let mh = if m.mag == 1 { 0.08 } else { 0.05 };
        self.bx(cx, mag, Dark, p(0.0, -mh / 2.0, 0.0), v(0.034, mh, 0.085));
        if m.mag == 2 {
            self.bx(cx, mag, Accent, p(0.0, -mh - 0.006, 0.0), v(0.03, 0.012, 0.06));
        }
        match m.stock {
            2 => {
                self.bx(cx, r, Body, p(0.0, -0.005, -0.27), v(0.058, 0.09, 0.22));
                self.bx(cx, r, Accent, p(0.0, 0.052, -0.27), v(0.05, 0.03, 0.17));
                self.bx(cx, r, Dark, p(0.0, -0.015, -0.385), v(0.06, 0.14, 0.02));
            }
            3 => {
                self.bx(cx, r, Body, p(0.0, 0.0, -0.25), v(0.05, 0.06, 0.2));
                self.bx(cx, r, Body, p(0.0, -0.05, -0.33), v(0.044, 0.05, 0.12));
                self.bx(cx, r, Accent, p(0.0, 0.05, -0.27), v(0.044, 0.016, 0.14));
                self.rody(cx, r, Steel, p(0.0, 0.038, -0.23), 0.004, 0.02);
                self.rody(cx, r, Metal, p(0.0, -0.1, -0.31), 0.006, 0.07);
                self.bx(cx, r, Dark, p(0.0, -0.01, -0.37), v(0.052, 0.13, 0.016));
            }
            _ => {
                self.bx(cx, r, Body, p(0.0, 0.0, -0.26), v(0.05, 0.07, 0.2));
                self.bx(cx, r, Body, p(0.0, -0.045, -0.32), v(0.044, 0.05, 0.12));
                self.bx(cx, r, Accent, p(0.0, 0.048, -0.27), v(0.044, 0.026, 0.15));
                self.bx(cx, r, Dark, p(0.0, -0.012, -0.37), v(0.054, 0.13, 0.016));
            }
        }
        if m.under == 0 {
            for s in [-1.0, 1.0] {
                self.bx(cx, r, Metal, p(0.012 * s, -0.02, 0.44), v(0.008, 0.008, 0.18));
                self.bx(cx, r, Dark, p(0.012 * s, -0.02, 0.355), v(0.012, 0.012, 0.016));
            }
            self.bx(cx, r, Metal, p(0.0, -0.016, 0.52), v(0.034, 0.014, 0.022));
        }
        let (sight, ads) = self.optic(cx, r, m.optic.max(1), p(0.0, 0.072, 0.02));
        let muzzle = if m.muzzle == 0 {
            self.rod(cx, r, Metal, p(0.0, 0.04, 0.97), 0.017, 0.04);
            for i in 0..3 {
                self.bx(cx, r, Dark, p(0.0, 0.04, 0.96 + i as f32 * 0.01), v(0.036, 0.008, 0.004));
            }
            p(0.0, 0.04, 0.99)
        } else {
            self.muzzle_dev(cx, r, m.muzzle, p(0.0, 0.04, 0.95), 0.013, false)
        };
        self.flash(cx, r, muzzle);
        let support = self.under(cx, r, m.under, p(0.0, -0.014, 0.42), false).or(Some(p(0.0, -0.025, 0.4)));
        GunInfo { muzzle, sight, ads_dist: ads, grip: p(0.0, -0.045, -0.07), support, eject: Some(p(0.025, 0.045, 0.0)), zmin: 0.0, zmax: 0.38 }
    }

    fn pistol(&mut self, cx: &Ctx, r: Entity, m: &GunMods) -> GunInfo {
        use Role::*;
        let slide_at = p(0.0, 0.03, 0.04);
        let slide = self.node(r, Transform::from_translation(slide_at), Some(AnimKind::Slide), cx.vm);
        self.bx(cx, slide, Body, Vec3::ZERO, v(0.03, 0.032, 0.19));
        for i in 0..5 {
            for s in [-1.0, 1.0] {
                self.bx(cx, slide, Dark, p(0.0152 * s, 0.002, -0.07 + i as f32 * 0.009), v(0.002, 0.022, 0.004));
            }
        }
        self.bx(cx, slide, Dark, p(0.0152, 0.006, 0.02), v(0.002, 0.012, 0.035));
        self.bx(cx, slide, Accent, p(0.0, 0.0165, 0.04), v(0.012, 0.002, 0.08));
        let sight = if m.optic == 0 {
            self.bx(cx, slide, Dark, p(0.0, 0.02, 0.085), v(0.004, 0.008, 0.006));
            self.ball(cx, slide, Reticle, p(0.0, 0.022, 0.081), 0.0013);
            for s in [-1.0, 1.0] {
                self.bx(cx, slide, Dark, p(0.008 * s, 0.02, -0.085), v(0.01, 0.009, 0.008));
                self.ball(cx, slide, Reticle, p(0.007 * s, 0.021, -0.0805), 0.0012);
            }
            (slide_at + p(0.0, 0.024, -0.085), 0.2)
        } else {
            let (s, d) = self.optic(cx, slide, m.optic, p(0.0, 0.016, -0.02));
            (slide_at + s, d + 0.06)
        };
        self.bx(cx, r, Dark, p(0.0, 0.004, 0.06), v(0.028, 0.022, 0.16));
        for i in 0..3 {
            self.bx(cx, r, Metal, p(0.0, -0.008, 0.1 + i as f32 * 0.012), v(0.024, 0.004, 0.006));
        }
        self.rod(cx, r, Steel, p(0.0, 0.03, 0.137), 0.006, 0.01);
        self.bxr(cx, r, Dark, p(0.0, -0.05, -0.02), v(0.03, 0.1, 0.046), tilt(0.22));
        for s in [-1.0, 1.0] {
            self.bxr(cx, r, Accent, p(0.0155 * s, -0.05, -0.02), v(0.002, 0.07, 0.032), tilt(0.22));
        }
        self.bx(cx, r, Dark, p(0.0, -0.022, 0.04), v(0.008, 0.006, 0.05));
        self.bx(cx, r, Dark, p(0.0, -0.014, 0.064), v(0.008, 0.02, 0.006));
        self.bxr(cx, r, Steel, p(0.0, -0.012, 0.035), v(0.004, 0.016, 0.005), tilt(0.2));
        self.bx(cx, r, Metal, p(0.0, 0.022, -0.065), v(0.014, 0.016, 0.012));
        let mag = self.node(r, Transform { translation: p(0.0, -0.1, -0.035), rotation: tilt(0.22), ..default() }, Some(AnimKind::Mag), cx.vm);
        if m.mag == 1 {
            self.bx(cx, mag, Dark, p(0.0, -0.02, 0.0), v(0.026, 0.05, 0.04));
            self.bx(cx, mag, Accent, p(0.0, -0.047, 0.0), v(0.032, 0.008, 0.048));
        } else {
            self.bx(cx, mag, Dark, p(0.0, -0.004, 0.0), v(0.032, 0.01, 0.048));
            if m.mag == 2 {
                self.bx(cx, mag, Accent, p(0.0, -0.011, 0.0), v(0.034, 0.006, 0.05));
            }
        }
        let muzzle = self.muzzle_dev(cx, r, m.muzzle, p(0.0, 0.03, 0.142), 0.006, false);
        self.flash(cx, r, muzzle);
        self.under(cx, r, m.under, p(0.0, -0.007, 0.1), true);
        GunInfo {
            muzzle,
            sight: sight.0,
            ads_dist: sight.1,
            grip: p(0.0, -0.05, -0.02),
            support: Some(p(-0.022, -0.075, -0.005)),
            eject: Some(p(0.017, 0.036, 0.06)),
            zmin: 0.0,
            zmax: 0.1,
        }
    }

    fn rocket(&mut self, cx: &Ctx, r: Entity, m: &GunMods) -> GunInfo {
        use Role::*;
        self.rod(cx, r, Body, p(0.0, 0.05, 0.15), 0.045, 1.0);
        self.tube(cx, r, Metal, p(0.0, 0.05, 0.67), 0.05, 0.06, 0.008);
        self.tube(cx, r, Dark, p(0.0, 0.05, -0.39), 0.054, 0.1, 0.01);
        self.rod(cx, r, Dark, p(0.0, 0.05, -0.36), 0.044, 0.004);
        for f in [0.45, -0.15] {
            self.rod(cx, r, Accent, p(0.0, 0.05, f), 0.047, 0.03);
        }
        self.bx(cx, r, Hazard, p(0.0, 0.05, 0.6), v(0.0905, 0.006, 0.03));
        let tip = self.node(r, Transform::from_translation(p(0.0, 0.05, 0.69)), Some(AnimKind::RocketTip), cx.vm);
        self.rod(cx, tip, Warhead, p(0.0, 0.0, 0.0), 0.038, 0.04);
        self.cone_fwd(cx, tip, Warhead, p(0.0, 0.0, 0.08), 0.038, 0.12);
        self.rod(cx, tip, Hazard, p(0.0, 0.0, 0.02), 0.0385, 0.01);
        self.bxr(cx, r, Dark, p(0.0, -0.045, 0.0), v(0.034, 0.09, 0.044), tilt(0.2));
        self.bx(cx, r, Dark, p(0.0, 0.0, 0.02), v(0.04, 0.03, 0.12));
        self.bx(cx, r, Metal, p(0.0, -0.025, 0.05), v(0.008, 0.006, 0.05));
        self.bxr(cx, r, Dark, p(0.0, -0.04, 0.28), v(0.03, 0.08, 0.035), tilt(-0.1));
        self.bx(cx, r, Dark, p(0.0, 0.0, 0.28), v(0.03, 0.02, 0.05));
        self.bx(cx, r, Dark, p(0.0, -0.005, -0.12), v(0.05, 0.03, 0.18));
        let (sight, ads) = if m.optic == 3 {
            self.bx(cx, r, Metal, p(0.0, 0.098, 0.06), v(0.03, 0.01, 0.16));
            self.optic(cx, r, 3, p(0.0, 0.103, 0.06))
        } else {
            self.bx(cx, r, Metal, p(0.0, 0.097, 0.3), v(0.012, 0.012, 0.012));
            self.bx(cx, r, Metal, p(0.0, 0.106, 0.3), v(0.0035, 0.016, 0.004));
            self.iron_rear(cx, r, p(0.0, 0.112, -0.05));
            (p(0.0, 0.113, -0.05), 0.12)
        };
        GunInfo {
            muzzle: p(0.0, 0.05, 0.76),
            sight,
            ads_dist: ads,
            grip: p(0.0, -0.045, -0.005),
            support: Some(p(0.0, -0.035, 0.28)),
            eject: None,
            zmin: 0.0,
            zmax: 0.45,
        }
    }

    fn knife(&mut self, cx: &Ctx, r: Entity) -> GunInfo {
        use Role::*;
        let blade = if cx.skin == 0 { Steel } else { Body };
        self.rod(cx, r, Dark, p(0.0, 0.0, -0.03), 0.014, 0.11);
        for i in 0..3 {
            self.rod(cx, r, Accent, p(0.0, 0.0, -0.065 + i as f32 * 0.03), 0.0148, 0.008);
        }
        self.ball(cx, r, Metal, p(0.0, 0.0, -0.09), 0.016);
        self.bx(cx, r, Metal, p(0.0, 0.0, 0.03), v(0.014, 0.055, 0.008));
        self.bx(cx, r, blade, p(0.0, 0.004, 0.115), v(0.004, 0.03, 0.16));
        self.bxr(cx, r, blade, p(0.0, 0.001, 0.205), v(0.004, 0.024, 0.04), Quat::from_rotation_x(-0.45));
        self.bx(cx, r, Dark, p(0.0, 0.0195, 0.1), v(0.0046, 0.003, 0.13));
        for s in [-1.0, 1.0] {
            self.bx(cx, r, Dark, p(0.0021 * s, 0.009, 0.1), v(0.001, 0.005, 0.11));
        }
        GunInfo { muzzle: p(0.0, 0.0, 0.23), sight: p(0.0, 0.0, 0.0), ads_dist: 0.3, grip: p(0.0, 0.0, -0.03), support: None, eject: None, zmin: 0.0, zmax: 0.11 }
    }

    fn grenade(&mut self, cx: &Ctx, r: Entity) -> GunInfo {
        use Role::*;
        self.rody(cx, r, Body, Vec3::ZERO, 0.03, 0.05);
        self.ball(cx, r, Body, p(0.0, 0.025, 0.0), 0.03);
        self.ball(cx, r, Body, p(0.0, -0.025, 0.0), 0.03);
        for y in [-0.02, 0.0, 0.02] {
            self.rody(cx, r, Dark, p(0.0, y, 0.0), 0.0305, 0.003);
        }
        for k in 0..6 {
            let a = k as f32 * TAU / 6.0;
            self.bxr(cx, r, Dark, v(a.cos() * 0.0302, 0.0, a.sin() * 0.0302), v(0.003, 0.07, 0.003), Quat::from_rotation_y(-a));
        }
        self.rody(cx, r, Metal, p(0.0, 0.06, 0.0), 0.012, 0.022);
        self.bxr(cx, r, Metal, p(0.018, 0.035, 0.0), v(0.005, 0.06, 0.016), Quat::from_rotation_z(-0.25));
        self.bx(cx, r, Hazard, p(0.0, 0.071, 0.0), v(0.026, 0.003, 0.026));
        let c = p(-0.02, 0.064, 0.0);
        for k in 0..10 {
            let a = k as f32 * TAU / 10.0;
            self.bxr(cx, r, Steel, c + v(0.0, a.cos() * 0.011, a.sin() * 0.011), v(0.002, 0.002, 0.008), Quat::from_rotation_x(a));
        }
        GunInfo { muzzle: p(0.0, 0.0, 0.04), sight: Vec3::ZERO, ads_dist: 0.3, grip: p(0.0, -0.015, 0.0), support: None, eject: None, zmin: -0.04, zmax: 0.04 }
    }

    /// Gloved hands and sleeves for the first-person view.
    fn arms(&mut self, cx: &Ctx, root: Entity, info: &GunInfo, weapon: &str) {
        use Role::*;
        let arms = self.node(root, Transform::IDENTITY, None, true);
        self.commands.entity(arms).insert(crate::viewmodel::VmArms);
        let g = info.grip;
        let (hand, thumb) = if weapon == "grenade" { (v(0.06, 0.06, 0.07), 0.0) } else { (v(0.048, 0.075, 0.062), 1.0) };
        self.bx(cx, arms, Glove, g + v(0.004, -0.006, 0.006), hand);
        if thumb > 0.0 {
            self.bx(cx, arms, Glove, g + v(-0.026, 0.03, -0.012), v(0.016, 0.016, 0.045));
            self.bx(cx, arms, Glove, g + v(0.0, -0.012, -0.034), v(0.04, 0.05, 0.016));
        }
        let wrist = g + v(0.012, -0.03, 0.05);
        let elbow = g + v(0.11, -0.17, 0.34);
        self.seg(cx, arms, Sleeve, wrist, elbow, 0.078, 0.07);
        self.seg(cx, arms, Cuff, wrist - v(0.002, -0.004, 0.012), wrist + v(0.004, -0.006, 0.02), 0.084, 0.076);
        self.seg(cx, arms, Sleeve, elbow, elbow + v(0.07, -0.16, 0.32), 0.092, 0.086);
        if let Some(s) = info.support {
            let pistol = weapon == "pistol";
            let palm = if pistol { v(0.05, 0.05, 0.06) } else { v(0.058, 0.04, 0.085) };
            self.bx(cx, arms, Glove, s, palm);
            if !pistol {
                self.bx(cx, arms, Glove, s + v(0.026, 0.02, 0.0), v(0.012, 0.03, 0.075));
                self.bx(cx, arms, Glove, s + v(-0.026, 0.018, 0.01), v(0.012, 0.026, 0.05));
            }
            let wrist = s + v(-0.02, -0.02, 0.05);
            let elbow = s + if pistol { v(-0.14, -0.16, 0.3) } else { v(-0.17, -0.2, 0.3) };
            self.seg(cx, arms, Sleeve, wrist, elbow, 0.076, 0.068);
            self.seg(cx, arms, Cuff, wrist - v(-0.003, -0.004, 0.012), wrist + v(-0.006, -0.006, 0.02), 0.082, 0.074);
            self.seg(cx, arms, Sleeve, elbow, elbow + v(-0.05, -0.17, 0.3), 0.09, 0.084);
        }
    }
}

// ---------------------------------------------------------------- third person

/// Builds and aims the gun each soldier carries, matching what they hold.
#[allow(clippy::too_many_arguments)]
pub fn third_person_guns(
    mut kit: GunKit,
    data: Res<GameData>,
    armory: Res<Armory>,
    loadout: Res<Loadout>,
    look: Res<Look>,
    owners: Query<(&NetObject, Has<LocalPlayer>)>,
    mut held: Query<(Entity, &mut HeldWeapon, &ChildOf, &mut Transform)>,
) {
    for (e, mut h, parent, mut tr) in &mut held {
        let Ok((obj, local)) = owners.get(parent.parent()) else { continue };
        let (wid, packed, pitch) = if local {
            let id = loadout.current_id().to_string();
            let packed = armory.get(&id).pack();
            (id, packed, look.pitch)
        } else {
            let idx = obj.extra.get(1).copied().unwrap_or(2.0).max(0.0) as usize;
            let id = data.weapons.get(idx).map(|w| w.id.clone()).unwrap_or_else(|| "rifle".into());
            let packed = obj.extra.get(3).copied().unwrap_or(0.0).max(0.0) as u32;
            (id, packed, obj.extra.first().copied().unwrap_or(0.0))
        };
        tr.rotation = Quat::from_rotation_x(-pitch.clamp(-1.2, 1.2) * 0.85) * Quat::from_rotation_y(PI);
        let sig = (wid.clone(), packed);
        if h.sig.as_ref() == Some(&sig) {
            continue;
        }
        if let Some(old) = h.gun.take() {
            kit.commands.entity(old).despawn();
        }
        let root = kit.commands.spawn((Transform::IDENTITY, Visibility::default())).id();
        let mods = GunMods::unpack(packed).sanitized(&wid);
        kit.build_gun(root, &wid, mods, false, 0);
        kit.commands.entity(e).add_child(root);
        h.gun = Some(root);
        h.sig = Some(sig);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_round_trips() {
        let m = GunMods { optic: 4, muzzle: 3, under: 2, mag: 3, stock: 1, skin: 16, reticle: 5 };
        assert_eq!(GunMods::unpack(m.pack()), m);
        assert_eq!(m.pack() as f32 as u32, m.pack(), "exact through the f32 network field");
    }

    #[test]
    fn sanitize_and_defaults() {
        assert_eq!(GunMods::factory("sniper").optic, 4);
        let bad = GunMods { optic: 4, muzzle: 4, ..default() }.sanitized("pistol");
        assert_eq!((bad.optic, bad.muzzle), (0, 0));
    }

    #[test]
    fn attachments_change_stats() {
        let defs = frontline_sim::weapons::load();
        let rifle = defs.iter().find(|d| d.id == "rifle").unwrap();
        let (d, h) = apply(rifle, &GunMods { mag: 3, muzzle: 4, optic: 3, ..default() });
        assert!(d.mag > rifle.mag && d.reload > rifle.reload);
        assert!(h.recoil < 0.7 && h.scoped && h.zoom == 3.0);
    }
}
