//! Create's potato cannon (`PotatoCannonItem`, `PotatoCannonProjectileType`,
//! `PotatoProjectileEntity`): its ammunition and the projectile's flight, damage and recovery.
//!
//! Only the rules: the projectile is an entity of Create's own type, which Horizon cannot
//! register (NeoForge's `EntityType` registration, gap 3), so the cannon fires nothing yet.
//! The types are Create's data (`data/create/create/potato_projectile/type/*.json`).

/// `CEquipment.maxPotatoCannonShots`'s default: shots a backtank's air pays for.
pub const MAX_SHOTS: i32 = 200;
/// `PotatoProjectileEntity.build`: `sized(.25f, .25f)`.
pub const SIZE: f32 = 0.25;
/// `getDefaultProjectileRange`.
pub const RANGE: i32 = 15;

/// A projectile type (`PotatoCannonProjectileType`) with Create's defaults filled in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Type {
    pub id: &'static str,
    pub items: &'static [&'static str],
    pub reload_ticks: i32,
    pub damage: i32,
    pub split: i32,
    pub knockback: f32,
    pub drag: f32,
    pub velocity: f32,
    pub gravity: f32,
    pub sound_pitch: f32,
    pub sticky: bool,
    /// `drop_stack`: what is left where it lands (none: nothing).
    pub drop: Option<&'static str>,
    /// What its `pre_entity_hit` / `on_entity_hit` / `on_block_hit` does, as Create's data
    /// names it (none: nothing more).
    pub action: Option<&'static str>,
}

const DEFAULT: Type = Type {
    id: "",
    items: &[],
    reload_ticks: 10,
    damage: 1,
    split: 1,
    knockback: 1.0,
    drag: 0.99,
    velocity: 1.0,
    gravity: 1.0,
    sound_pitch: 1.0,
    sticky: false,
    drop: None,
    action: None,
};

macro_rules! t {
    ($id:literal, [$($i:literal),*] $(, $f:ident: $v:expr)* $(,)?) => {
        Type { id: $id, items: &[$($i),*], $($f: $v,)* ..DEFAULT }
    };
}

/// Create's types in the registry's order (by id).
pub const TYPES: &[Type] = &[
    t!("apple", ["minecraft:apple"], damage: 5, knockback: 0.5, sound_pitch: 1.1, velocity: 1.45),
    t!("baked_potato", ["minecraft:baked_potato"], damage: 5, knockback: 0.5, reload_ticks: 15,
        velocity: 1.25, action: Some("set_on_fire 60")),
    t!("beetroot", ["minecraft:beetroot"], damage: 2, knockback: 0.1, reload_ticks: 5,
        sound_pitch: 1.6, velocity: 1.6),
    t!("blaze_cake", ["create:blaze_cake"], damage: 15, knockback: 0.3, reload_ticks: 20,
        sticky: true, velocity: 1.1, action: Some("set_on_fire 240")),
    t!("cake", ["minecraft:cake"], damage: 8, knockback: 0.1, reload_ticks: 15, sticky: true,
        velocity: 1.1),
    t!("carrot", ["minecraft:carrot"], damage: 4, knockback: 0.3, reload_ticks: 12,
        sound_pitch: 1.5, velocity: 1.45, action: Some("plant_crop minecraft:carrots")),
    t!("chocolate_berry", ["create:chocolate_glazed_berries"], damage: 4, knockback: 0.2,
        sound_pitch: 1.25, split: 3, velocity: 1.05),
    t!("chorus_fruit", ["minecraft:chorus_fruit"], damage: 3, knockback: 0.05, reload_ticks: 15,
        velocity: 1.2, action: Some("chorus_teleport 20")),
    t!("enchanted_golden_apple", ["minecraft:enchanted_golden_apple"], knockback: 0.05,
        reload_ticks: 100, sound_pitch: 1.1, velocity: 1.45, action: Some("food_effects")),
    t!("fallback", [], damage: 0),
    t!("fish", ["minecraft:cod", "minecraft:cooked_cod", "minecraft:salmon",
        "minecraft:cooked_salmon", "minecraft:tropical_fish"], damage: 4, knockback: 0.6,
        sound_pitch: 1.3, sticky: true, velocity: 1.3),
    t!("glistering_melon", ["minecraft:glistering_melon_slice"], damage: 5, knockback: 0.1,
        reload_ticks: 8, sound_pitch: 1.5, velocity: 1.45,
        action: Some("potion_effect minecraft:glowing 1 100")),
    t!("glow_berry", ["minecraft:glow_berries"], damage: 2, knockback: 0.05, sound_pitch: 1.2,
        split: 2, velocity: 1.05, action: Some("potion_effect minecraft:glowing 1 200")),
    t!("golden_apple", ["minecraft:golden_apple"], knockback: 0.05, reload_ticks: 100,
        sound_pitch: 1.1, velocity: 1.45, action: Some("cure_zombie_villager")),
    t!("golden_carrot", ["minecraft:golden_carrot"], damage: 12, knockback: 0.5,
        reload_ticks: 15, sound_pitch: 1.5, velocity: 1.45),
    t!("honeyed_apple", ["create:honeyed_apple"], damage: 6, knockback: 0.1, reload_ticks: 15,
        sound_pitch: 1.1, velocity: 1.35, action: Some("potion_effect minecraft:slowness 2 160")),
    t!("melon_block", ["minecraft:melon"], damage: 8, knockback: 2.0, reload_ticks: 20,
        sound_pitch: 0.9, velocity: 0.95, action: Some("place_block_on_ground minecraft:melon")),
    t!("melon_slice", ["minecraft:melon_slice"], damage: 3, knockback: 0.1, reload_ticks: 8,
        sound_pitch: 1.5, velocity: 1.45),
    t!("poison_potato", ["minecraft:poisonous_potato"], damage: 5, knockback: 0.05,
        reload_ticks: 15, velocity: 1.25, action: Some("potion_effect minecraft:poison 1 160")),
    t!("potato", ["minecraft:potato"], damage: 5, knockback: 1.5, reload_ticks: 15,
        velocity: 1.25, action: Some("plant_crop minecraft:potatoes")),
    t!("pufferfish", ["minecraft:pufferfish"], damage: 4, knockback: 0.4, sound_pitch: 1.1,
        sticky: true, velocity: 1.1, action: Some("food_effects")),
    t!("pumpkin_block", ["minecraft:pumpkin"], damage: 6, knockback: 2.0, reload_ticks: 15,
        sound_pitch: 0.9, velocity: 0.95, action: Some("place_block_on_ground minecraft:pumpkin")),
    t!("pumpkin_pie", ["minecraft:pumpkin_pie"], damage: 7, knockback: 0.05, reload_ticks: 15,
        sound_pitch: 1.1, sticky: true, velocity: 1.1),
    t!("suspicious_stew", ["minecraft:suspicious_stew"], damage: 3, knockback: 0.2,
        reload_ticks: 40, velocity: 0.8, drop: Some("minecraft:bowl"),
        action: Some("suspicious_stew")),
    t!("sweet_berry", ["minecraft:sweet_berries"], damage: 3, knockback: 0.1, sound_pitch: 1.25,
        split: 3, velocity: 1.05),
];

/// `getTypeForItem`: the first type listing the item (none: not ammunition).
pub fn type_for(item: &str) -> Option<&'static Type> {
    TYPES.iter().find(|t| t.items.contains(&item))
}

/// `setItem`'s fallback for an item no type lists.
pub fn fallback() -> &'static Type {
    TYPES.iter().find(|t| t.id == "fallback").expect("fallback")
}

fn norm(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-4 { [0.0; 3] } else { v.map(|c| c / l) }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `ShootableGadgetItemMethods.getGunBarrelVec`: a point before the eyes, the right hand's
/// `forward` mirrored for the left one (`flip`), turned by pitch then yaw (degrees).
pub fn barrel(
    eye: [f64; 3],
    yaw: f32,
    pitch: f32,
    right_hand: bool,
    forward: [f64; 3],
) -> [f64; 3] {
    let yaw = (f64::from(yaw) / -180.0 * std::f64::consts::PI) as f32;
    let pitch = (f64::from(pitch) / -180.0 * std::f64::consts::PI) as f32;
    let flip = if right_hand { -1.0 } else { 1.0 };
    let v = [flip * forward[0], forward[1], forward[2]];
    // `Vec3.xRot`, then `yRot`.
    let (c, s) = (f64::from(pitch.cos()), f64::from(pitch.sin()));
    let v = [v[0], v[1] * c + v[2] * s, v[2] * c - v[1] * s];
    let (c, s) = (f64::from(yaw.cos()), f64::from(yaw.sin()));
    let v = [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s];
    [eye[0] + v[0], eye[1] + v[1], eye[2] + v[2]]
}

/// `PotatoCannonItem.use`'s motion: the look corrected toward the barrel, normalized, × 2 × the
/// type's velocity.
pub fn motion(look: [f64; 3], correction: [f64; 3], t: &Type) -> [f64; 3] {
    let n = norm([
        look[0] + correction[0],
        look[1] + correction[1],
        look[2] + correction[2],
    ]);
    n.map(|c| c * 2.0 * f64::from(t.velocity))
}

/// `VecHelper.lookAt`: `vec` in the frame whose forward is `fwd`.
fn look_at(vec: [f64; 3], fwd: [f64; 3]) -> [f64; 3] {
    let fwd = norm(fwd);
    let mut up = [0.0, 1.0, 0.0];
    let dot = fwd[1];
    if dot.abs() > 1.0 - 1.0e-3 {
        up = [0.0, 0.0, if dot > 0.0 { 1.0 } else { -1.0 }];
    }
    let right = norm(cross(fwd, up));
    let up = norm(cross(right, fwd));
    [0, 1, 2].map(|i| vec[0] * right[i] + vec[1] * up[i] + vec[2] * fwd[i])
}

/// `VecHelper.rotate(vec, deg, Z)`.
fn rotate_z(v: [f64; 3], deg: f64) -> [f64; 3] {
    if deg == 0.0 {
        return v;
    }
    let a = (deg / 180.0 * std::f64::consts::PI) as f32;
    let (s, c) = (f64::from(a.sin()), f64::from(a.cos()));
    [v[0] * c - v[1] * s, v[1] * c + v[0] * s, v[2]]
}

/// The motions of a shot's projectiles: one, or `split` spread around the motion
/// (`sprayBase` turned `base_deg` (`360 × nextFloat`), each turned `360 / split` on plus its `imperfection`).
pub fn spray(motion: [f64; 3], t: &Type, base_deg: f32, imperfections: &[f32]) -> Vec<[f64; 3]> {
    if t.split <= 1 {
        return vec![motion];
    }
    let base = rotate_z([0.0, 0.1, 0.0], f64::from(base_deg));
    let change = 360.0 / t.split as f32;
    (0..t.split as usize)
        .map(|i| {
            let imperfection = imperfections.get(i).copied().unwrap_or(0.0);
            let offset = rotate_z(base, f64::from(i as f32 * change + imperfection));
            let o = look_at(offset, motion);
            [motion[0] + o[0], motion[1] + o[1], motion[2] + o[2]]
        })
        .collect()
}

/// `PotatoProjectileEntity.tick` in flight: gravity × the type's, then its drag
/// (`getInertia` 1: the hurting projectile's own drag is none).
pub fn fly(motion: [f64; 3], t: &Type) -> [f64; 3] {
    let m = [
        motion[0],
        motion[1] - 0.05 * f64::from(t.gravity),
        motion[2],
    ];
    m.map(|c| c * f64::from(t.drag))
}

/// `setEnchantmentEffectsFromCannon`: the chance a landed shot gives its item back.
pub fn recovery_chance(level: i32) -> f32 {
    if level > 0 {
        0.125 + level as f32 * 0.125
    } else {
        0.0
    }
}

/// `onHitEntity`: the damage and knockback a hit deals, and the target's invulnerability after
/// it when the cannon reloads faster than 10 ticks.
pub fn hit(t: &Type) -> (f32, f32, Option<i32>) {
    let invulnerable = (t.reload_ticks < 10).then_some(t.reload_ticks + 10);
    (t.damage as f32, t.knockback, invulnerable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ammunition_is_found_by_item() {
        let potato = type_for("minecraft:potato").unwrap();
        assert_eq!((potato.damage, potato.reload_ticks), (5, 15));
        assert_eq!(potato.knockback, 1.5);
        assert_eq!(potato.drag, 0.99);
        assert_eq!(type_for("minecraft:salmon").unwrap().id, "fish");
        assert!(type_for("minecraft:dirt").is_none());
        assert_eq!(fallback().damage, 0);
        assert_eq!(
            type_for("minecraft:suspicious_stew").unwrap().drop,
            Some("minecraft:bowl")
        );
        // Each item in one type only.
        let mut all: Vec<&str> = TYPES.iter().flat_map(|t| t.items.iter().copied()).collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
        assert_eq!(TYPES.len(), 25);
    }

    #[test]
    fn shots_leave_the_barrel_and_fall() {
        // Looking south (yaw 0) level: the right hand's barrel .75 left of the eye, 1.5 ahead.
        let b = barrel([0.0, 1.62, 0.0], 0.0, 0.0, true, [0.75, -0.15, 1.5]);
        assert!(
            (b[0] + 0.75).abs() < 1e-6 && (b[1] - 1.47).abs() < 1e-6 && (b[2] - 1.5).abs() < 1e-6
        );
        let potato = type_for("minecraft:potato").unwrap();
        let m = motion([0.0, 0.0, 1.0], [0.0; 3], potato);
        assert!((m[2] - 2.5).abs() < 1e-9);
        let next = fly(m, potato);
        assert!((next[1] + 0.05 * 0.99).abs() < 1e-6 && (next[2] - 2.5 * 0.99).abs() < 1e-6);
        // Split shots fan out around the motion, each about 0.1 off it.
        let berry = type_for("minecraft:sweet_berries").unwrap();
        let s = spray(m, berry, 0.0, &[0.0, 0.0, 0.0]);
        assert_eq!(s.len(), 3);
        for v in &s {
            let d = [v[0] - m[0], v[1] - m[1], v[2] - m[2]];
            assert!(((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() - 0.1).abs() < 1e-6);
        }
        assert_eq!(spray(m, potato, 0.0, &[]), vec![m]);
        assert_eq!(recovery_chance(0), 0.0);
        assert_eq!(recovery_chance(3), 0.5);
        assert_eq!(
            hit(type_for("minecraft:beetroot").unwrap()),
            (2.0, 0.1, Some(15))
        );
        assert_eq!(hit(potato).2, None);
    }
}
