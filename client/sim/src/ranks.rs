//! XP ranks. Rank only rises from XP; the host may set it directly.

const RANKS: [(&str, i64); 10] = [
    ("Recruit", 0),
    ("Private", 100),
    ("Corporal", 300),
    ("Sergeant", 700),
    ("Staff Sergeant", 1_500),
    ("Lieutenant", 3_000),
    ("Captain", 6_000),
    ("Major", 10_000),
    ("Colonel", 16_000),
    ("General", 25_000),
];

/// XP needed for `rank`; ranks past General need 12k more each.
pub fn xp_for(rank: u32) -> i64 {
    let r = rank as usize;
    if r < RANKS.len() {
        RANKS[r].1
    } else {
        RANKS[RANKS.len() - 1].1 + (r - RANKS.len() + 1) as i64 * 12_000
    }
}

pub fn rank_for_xp(xp: i64) -> u32 {
    let mut r = 0;
    while xp >= xp_for(r + 1) && r < 100 {
        r += 1;
    }
    r
}

pub fn name(rank: u32) -> String {
    let r = rank as usize;
    if r < RANKS.len() {
        RANKS[r].0.to_string()
    } else {
        format!("General {}", "★".repeat(((r - RANKS.len() + 1).min(5)) as usize))
    }
}

/// Rank after gaining XP: never lowers a rank the host granted.
pub fn apply_xp(rank: u32, xp: i64) -> u32 {
    rank.max(rank_for_xp(xp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds() {
        assert_eq!(rank_for_xp(0), 0);
        assert_eq!(rank_for_xp(99), 0);
        assert_eq!(rank_for_xp(100), 1);
        assert_eq!(rank_for_xp(25_000), 9);
        assert_eq!(rank_for_xp(37_000), 10);
        assert_eq!(name(10), "General ★");
        assert_eq!(apply_xp(5, 0), 5);
    }
}
