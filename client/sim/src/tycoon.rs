//! Base items, purchases and rebirths. The client owns its economy; the server
//! only stores the resulting `BaseState` JSON.

use std::collections::{BTreeMap, HashSet};

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

pub const ITEMS_RON: &str = include_str!("../../assets/tycoon/items.ron");

#[derive(Debug, Clone, Deserialize)]
pub struct LineDef {
    pub start: (f32, f32),
    pub end: (f32, f32),
    pub collector: (f32, f32),
}

#[derive(Debug, Clone, Deserialize)]
pub enum ItemKind {
    Line { line: usize },
    Dropper { line: usize, value: f32, interval: f32 },
    Upgrader { line: usize, mult: f32 },
    Wall { size: (f32, f32, f32) },
    Walls { parts: Vec<((f32, f32, f32), (f32, f32, f32))> },
    Tower,
    Turret { damage: f32, range: f32, rate: f32 },
    WeaponRack { weapon: String },
    Barracks { max_health: f32 },
    MedStation,
    ArmorDepot { armor: f32 },
    VehiclePad { vehicle: String },
    Headquarters,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    pub cost: i64,
    pub requires: Vec<String>,
    pub rank: u32,
    pub kind: ItemKind,
    pub pos: (f32, f32, f32),
    pub button: (f32, f32),
}

impl ItemDef {
    pub fn pos(&self) -> Vec3 {
        Vec3::new(self.pos.0, self.pos.1, self.pos.2)
    }
    pub fn button(&self) -> Vec2 {
        Vec2::new(self.button.0, self.button.1)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub lines: Vec<LineDef>,
    pub claim_pad: (f32, f32),
    pub items: Vec<ItemDef>,
}

/// Persisted per player per room (the server stores it as `base_json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BaseState {
    #[serde(default)]
    pub owned: Vec<String>,
    /// Kills per weapon id in this room, for mastery challenges.
    #[serde(default)]
    pub weapon_kills: BTreeMap<String, u32>,
    /// Mastery rewards already paid out, as "weapon:tier".
    #[serde(default)]
    pub claimed: Vec<String>,
}

impl BaseState {
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
    pub fn owns(&self, id: &str) -> bool {
        self.owned.iter().any(|o| o == id)
    }
    pub fn owned_set(&self) -> HashSet<&str> {
        self.owned.iter().map(String::as_str).collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BuyError {
    Unknown,
    AlreadyOwned,
    Missing(String),
    Rank(u32),
    Cash(i64),
    Kills(u32),
}

impl std::fmt::Display for BuyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuyError::Unknown => write!(f, "unknown item"),
            BuyError::AlreadyOwned => write!(f, "already owned"),
            BuyError::Missing(id) => write!(f, "requires {id}"),
            BuyError::Rank(r) => write!(f, "requires rank {}", crate::ranks::name(*r)),
            BuyError::Cash(short) => write!(f, "need ${short} more"),
            BuyError::Kills(n) => write!(f, "needs {n} kills in this room"),
        }
    }
}

impl Catalog {
    pub fn load() -> Self {
        ron::from_str(ITEMS_RON).expect("assets/tycoon/items.ron")
    }

    pub fn get(&self, id: &str) -> Option<&ItemDef> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Items whose requirements are met but which are not owned yet (rank-locked included).
    pub fn available<'a>(&'a self, base: &'a BaseState) -> impl Iterator<Item = &'a ItemDef> + 'a {
        let owned = base.owned_set();
        self.items
            .iter()
            .filter(move |i| !owned.contains(i.id.as_str()) && i.requires.iter().all(|r| owned.contains(r.as_str())))
    }

    pub fn can_buy(&self, id: &str, base: &BaseState, cash: i64, rank: u32) -> Result<&ItemDef, BuyError> {
        let item = self.get(id).ok_or(BuyError::Unknown)?;
        if base.owns(id) {
            return Err(BuyError::AlreadyOwned);
        }
        if let Some(missing) = item.requires.iter().find(|r| !base.owns(r)) {
            return Err(BuyError::Missing(missing.clone()));
        }
        if rank < item.rank {
            return Err(BuyError::Rank(item.rank));
        }
        if cash < item.cost {
            return Err(BuyError::Cash(item.cost - cash));
        }
        Ok(item)
    }

    /// Combat kills in the room needed before an item can be built: the heavier
    /// weapons and vehicles are earned on the battlefield as well as bought.
    pub fn kills_required(&self, id: &str) -> u32 {
        match self.get(id).map(|i| &i.kind) {
            Some(ItemKind::WeaponRack { weapon }) => match weapon.as_str() {
                "shotgun" => 5,
                "sniper" => 15,
                "rocket" => 30,
                _ => 0,
            },
            Some(ItemKind::VehiclePad { vehicle }) => match vehicle.as_str() {
                "heli" => 25,
                "tank" => 40,
                "plane" => 60,
                _ => 0,
            },
            _ => 0,
        }
    }

    /// Fails if the player hasn't earned the item yet.
    pub fn check_kills(&self, id: &str, kills: i64) -> Result<(), BuyError> {
        let need = self.kills_required(id);
        if kills < need as i64 { Err(BuyError::Kills(need)) } else { Ok(()) }
    }

    /// Validates and applies a purchase; returns the new cash.
    pub fn buy(&self, id: &str, base: &mut BaseState, cash: i64, rank: u32) -> Result<i64, BuyError> {
        let cost = self.can_buy(id, base, cash, rank)?.cost;
        base.owned.push(id.to_string());
        Ok(cash - cost)
    }

    /// Drops any owned ids that no longer exist or whose requirements are gone
    /// (e.g. after a catalog change), keeping saves loadable.
    pub fn sanitize(&self, base: &mut BaseState) {
        let mut kept: Vec<String> = Vec::new();
        let mut changed = true;
        let mut pending: Vec<String> = base.owned.clone();
        while changed {
            changed = false;
            pending.retain(|id| {
                let Some(item) = self.get(id) else { return false };
                if kept.contains(id) {
                    return false;
                }
                if item.requires.iter().all(|r| kept.contains(r)) {
                    kept.push(id.clone());
                    changed = true;
                    false
                } else {
                    true
                }
            });
        }
        base.owned = kept;
    }

    pub fn owns_kind<'a>(&'a self, base: &'a BaseState) -> impl Iterator<Item = &'a ItemDef> + 'a {
        base.owned.iter().filter_map(|id| self.get(id))
    }

    /// Steady-state income per second (before multipliers), for the HUD.
    pub fn income_per_sec(&self, base: &BaseState) -> f32 {
        let mut total = 0.0;
        for (li, _) in self.lines.iter().enumerate() {
            let mut mult = 1.0;
            let mut sum = 0.0;
            for item in self.owns_kind(base) {
                match item.kind {
                    ItemKind::Dropper { line, value, interval } if line == li => sum += value / interval,
                    ItemKind::Upgrader { line, mult: m } if line == li => mult *= m,
                    _ => {}
                }
            }
            total += sum * mult;
        }
        total
    }

    pub fn has_line(&self, base: &BaseState, line: usize) -> bool {
        self.owns_kind(base).any(|i| matches!(i.kind, ItemKind::Line { line: l } if l == line))
    }
}

/// Weapon mastery tiers: (kills with the weapon, cash, xp).
pub const MASTERY: [(u32, i64, i64); 3] = [(10, 500, 150), (40, 2500, 600), (120, 10000, 2000)];

/// Counts a kill with `weapon` and returns the (tier, cash, xp) of every tier it completes.
pub fn record_kill(base: &mut BaseState, weapon: &str) -> Vec<(usize, i64, i64)> {
    let n = base.weapon_kills.entry(weapon.to_string()).or_insert(0);
    *n += 1;
    let n = *n;
    let mut done = Vec::new();
    for (i, (need, cash, xp)) in MASTERY.iter().enumerate() {
        let key = format!("{weapon}:{i}");
        if n >= *need && !base.claimed.contains(&key) {
            base.claimed.push(key);
            done.push((i, *cash, *xp));
        }
    }
    done
}

pub fn rebirth_price(rebirths: u32) -> i64 {
    (300_000.0 * 1.6f64.powi(rebirths as i32)) as i64
}

/// Permanent income multiplier from rebirths.
pub fn rebirth_multiplier(rebirths: u32) -> f32 {
    1.0 + 0.25 * rebirths as f32
}

pub fn can_rebirth(base: &BaseState, cash: i64, rebirths: u32) -> bool {
    base.owns("hq") && cash >= rebirth_price(rebirths)
}

/// XP granted for buying an item.
pub fn purchase_xp(cost: i64) -> i64 {
    (cost / 100).clamp(5, 500)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        let c = Catalog::load();
        let mut seen = HashSet::new();
        for i in &c.items {
            assert!(seen.insert(i.id.clone()), "duplicate id {}", i.id);
            for r in &i.requires {
                assert!(c.get(r).is_some(), "{} requires unknown {}", i.id, r);
            }
            match &i.kind {
                ItemKind::Dropper { line, .. } | ItemKind::Upgrader { line, .. } | ItemKind::Line { line } => {
                    assert!(*line < c.lines.len(), "{} on missing line", i.id)
                }
                _ => {}
            }
        }
        // Everything is reachable by buying in some order with enough cash and rank.
        let mut base = BaseState::default();
        loop {
            let next: Vec<String> = c.available(&base).map(|i| i.id.clone()).collect();
            if next.is_empty() {
                break;
            }
            for id in next {
                c.buy(&id, &mut base, i64::MAX, 100).unwrap();
            }
        }
        assert_eq!(base.owned.len(), c.items.len(), "all items reachable");
    }

    #[test]
    fn buying_rules() {
        let c = Catalog::load();
        let mut base = BaseState::default();
        assert_eq!(c.buy("dropper_2", &mut base, 1000, 0), Err(BuyError::Missing("dropper_1".into())));
        let cash = c.buy("starter_line", &mut base, 100, 0).unwrap();
        let cash = c.buy("dropper_1", &mut base, cash, 0).unwrap();
        assert_eq!(cash, 100);
        assert_eq!(c.buy("dropper_2", &mut base, cash, 0), Err(BuyError::Cash(50)));
        assert_eq!(c.buy("dropper_1", &mut base, cash, 0), Err(BuyError::AlreadyOwned));
        let cash = c.buy("dropper_2", &mut base, 150, 0).unwrap();
        assert_eq!(cash, 0);
        assert!(c.income_per_sec(&base) > 4.9);
    }

    #[test]
    fn rank_gate() {
        let c = Catalog::load();
        let mut base = BaseState::default();
        for id in ["starter_line", "dropper_1", "dropper_2", "dropper_3", "upgrader_1", "dropper_4"] {
            c.buy(id, &mut base, i64::MAX, 0).unwrap();
        }
        assert_eq!(c.buy("dropper_5", &mut base, i64::MAX, 0), Err(BuyError::Rank(1)));
    }

    #[test]
    fn sanitize_drops_orphans() {
        let c = Catalog::load();
        let mut base = BaseState { owned: vec!["dropper_2".into(), "ghost".into(), "starter_line".into(), "dropper_1".into()], ..Default::default() };
        c.sanitize(&mut base);
        assert_eq!(base.owned, vec!["starter_line", "dropper_1", "dropper_2"]);
        let mut orphan = BaseState { owned: vec!["dropper_2".into()], ..Default::default() };
        c.sanitize(&mut orphan);
        assert!(orphan.owned.is_empty());
    }

    #[test]
    fn json_roundtrip() {
        let b = BaseState { owned: vec!["a".into()], ..Default::default() };
        assert_eq!(BaseState::from_json(&b.to_json()), b);
        assert_eq!(BaseState::from_json("{}"), BaseState::default());
        assert_eq!(BaseState::from_json("garbage"), BaseState::default());
    }

    #[test]
    fn rebirth() {
        let base = BaseState { owned: vec!["hq".into()], ..Default::default() };
        assert!(!can_rebirth(&base, 1000, 0));
        assert!(can_rebirth(&base, rebirth_price(0), 0));
        assert!(rebirth_price(1) > rebirth_price(0));
        assert_eq!(rebirth_multiplier(2), 1.5);
    }

    #[test]
    fn mastery_pays_once_per_tier() {
        let mut b = BaseState::default();
        for _ in 0..9 {
            assert!(record_kill(&mut b, "rifle").is_empty());
        }
        assert_eq!(record_kill(&mut b, "rifle").len(), 1);
        assert!(record_kill(&mut b, "rifle").is_empty());
        assert!(record_kill(&mut b, "smg").is_empty());
        assert_eq!(BaseState::from_json(&b.to_json()), b);
    }

    #[test]
    fn heavy_items_need_kills() {
        let c = Catalog::load();
        assert_eq!(c.kills_required("starter_line"), 0);
        let tank = c.items.iter().find(|i| matches!(&i.kind, ItemKind::VehiclePad { vehicle } if vehicle == "tank")).unwrap();
        assert_eq!(c.check_kills(&tank.id, 10), Err(BuyError::Kills(40)));
        assert!(c.check_kills(&tank.id, 40).is_ok());
    }
}
