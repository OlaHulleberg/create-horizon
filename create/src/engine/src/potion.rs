//! A potion's contents (`PotionContents`: its potion, custom colour, custom name and custom
//! effects), as Create's potion fluid carries them (`DataComponents.POTION_CONTENTS` on the
//! `FluidStack`, `PotionFluid.of`), with the game's effect and potion tables they are coloured
//! and named by (`MobEffects`, `Potions`, `PotionContents.getColor`, `Potion.getName`): vanilla
//! facts the game shows (an effect's particle colour, a potion's effects and durations in its
//! tooltip), which the mod API does not give; tabulated as arismoko/horizon (MIT) has them
//! (`THIRD_PARTY_NOTICES.md`).
//!
//! Contents are interned: a potion fluid holds a `&'static Contents`, so `tank::Fluid` stays a
//! plain value; equal contents are one entry. They are saved and sent whole ([`Contents::encode`]).

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// `PotionContents`. Two are the same potion fluid only when all of it is the same
/// (`FluidStack.isSameFluidSameComponents`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Contents {
    /// The registered potion (`minecraft:swiftness`).
    pub potion: Option<String>,
    /// `custom_color`, ARGB as saved.
    pub custom_color: Option<i32>,
    /// `custom_name`: the name's suffix (`item.minecraft.potion.effect.<name>`).
    pub custom_name: Option<String>,
    /// `custom_effects` as the stack held them, kept whole (the list tag's network bytes; empty
    /// for none). The glue reads and writes them.
    pub custom_effects: Vec<u8>,
    /// What of `custom_effects` colours the potion: each effect's id, amplifier and whether it
    /// shows its particles (`MobEffectInstance.isVisible`).
    pub effects: Vec<Effect>,
}

/// A custom effect as it colours a potion.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Effect {
    pub id: String,
    pub amplifier: i32,
    pub visible: bool,
}

/// `PotionContents.BASE_POTION_COLOR` (-13083194) as RGB.
pub const BASE_COLOR: u32 = 0x385DC6;

/// `MobEffects` and their colours (`MobEffect.getColor`), vanilla's ids without namespace.
pub const EFFECTS: [(&str, u32); 40] = [
    ("speed", 3402751),
    ("slowness", 9154528),
    ("haste", 14270531),
    ("mining_fatigue", 4866583),
    ("strength", 16762624),
    ("instant_health", 16262179),
    ("instant_damage", 11101546),
    ("jump_boost", 16646020),
    ("nausea", 5578058),
    ("regeneration", 13458603),
    ("resistance", 9520880),
    ("fire_resistance", 16750848),
    ("water_breathing", 10017472),
    ("invisibility", 16185078),
    ("blindness", 2039587),
    ("night_vision", 12779366),
    ("hunger", 5797459),
    ("weakness", 4738376),
    ("poison", 8889187),
    ("wither", 7561558),
    ("health_boost", 16284963),
    ("absorption", 2445989),
    ("saturation", 16262179),
    ("glowing", 9740385),
    ("levitation", 13565951),
    ("luck", 5882118),
    ("unluck", 12624973),
    ("slow_falling", 15978425),
    ("conduit_power", 1950417),
    ("dolphins_grace", 8954814),
    ("bad_omen", 745784),
    ("hero_of_the_village", 4521796),
    ("darkness", 2696993),
    ("trial_omen", 1484454),
    ("raid_omen", 14565464),
    ("wind_charged", 12438015),
    ("weaving", 7891290),
    ("oozing", 10092451),
    ("infested", 9214860),
    ("breath_of_the_nautilus", 65518),
];

/// A potion: id, name suffix and effects (effect, amplifier, duration in ticks).
pub type Potion = (
    &'static str,
    &'static str,
    &'static [(&'static str, i32, i32)],
);

/// The registered potions (`minecraft:potion`, vanilla's order): id, name suffix
/// (`Potion.name`) and effects (effect, amplifier, duration in ticks).
pub const POTIONS: [Potion; 46] = [
    ("water", "water", &[]),
    ("mundane", "mundane", &[]),
    ("thick", "thick", &[]),
    ("awkward", "awkward", &[]),
    ("night_vision", "night_vision", &[("night_vision", 0, 3600)]),
    (
        "long_night_vision",
        "night_vision",
        &[("night_vision", 0, 9600)],
    ),
    ("invisibility", "invisibility", &[("invisibility", 0, 3600)]),
    (
        "long_invisibility",
        "invisibility",
        &[("invisibility", 0, 9600)],
    ),
    ("leaping", "leaping", &[("jump_boost", 0, 3600)]),
    ("long_leaping", "leaping", &[("jump_boost", 0, 9600)]),
    ("strong_leaping", "leaping", &[("jump_boost", 1, 1800)]),
    (
        "fire_resistance",
        "fire_resistance",
        &[("fire_resistance", 0, 3600)],
    ),
    (
        "long_fire_resistance",
        "fire_resistance",
        &[("fire_resistance", 0, 9600)],
    ),
    ("swiftness", "swiftness", &[("speed", 0, 3600)]),
    ("long_swiftness", "swiftness", &[("speed", 0, 9600)]),
    ("strong_swiftness", "swiftness", &[("speed", 1, 1800)]),
    ("slowness", "slowness", &[("slowness", 0, 1800)]),
    ("long_slowness", "slowness", &[("slowness", 0, 4800)]),
    ("strong_slowness", "slowness", &[("slowness", 3, 400)]),
    (
        "turtle_master",
        "turtle_master",
        &[("slowness", 3, 400), ("resistance", 2, 400)],
    ),
    (
        "long_turtle_master",
        "turtle_master",
        &[("slowness", 3, 800), ("resistance", 2, 800)],
    ),
    (
        "strong_turtle_master",
        "turtle_master",
        &[("slowness", 5, 400), ("resistance", 3, 400)],
    ),
    (
        "water_breathing",
        "water_breathing",
        &[("water_breathing", 0, 3600)],
    ),
    (
        "long_water_breathing",
        "water_breathing",
        &[("water_breathing", 0, 9600)],
    ),
    ("healing", "healing", &[("instant_health", 0, 1)]),
    ("strong_healing", "healing", &[("instant_health", 1, 1)]),
    ("harming", "harming", &[("instant_damage", 0, 1)]),
    ("strong_harming", "harming", &[("instant_damage", 1, 1)]),
    ("poison", "poison", &[("poison", 0, 900)]),
    ("long_poison", "poison", &[("poison", 0, 1800)]),
    ("strong_poison", "poison", &[("poison", 1, 432)]),
    ("regeneration", "regeneration", &[("regeneration", 0, 900)]),
    (
        "long_regeneration",
        "regeneration",
        &[("regeneration", 0, 1800)],
    ),
    (
        "strong_regeneration",
        "regeneration",
        &[("regeneration", 1, 450)],
    ),
    ("strength", "strength", &[("strength", 0, 3600)]),
    ("long_strength", "strength", &[("strength", 0, 9600)]),
    ("strong_strength", "strength", &[("strength", 1, 1800)]),
    ("weakness", "weakness", &[("weakness", 0, 1800)]),
    ("long_weakness", "weakness", &[("weakness", 0, 4800)]),
    ("luck", "luck", &[("luck", 0, 6000)]),
    ("slow_falling", "slow_falling", &[("slow_falling", 0, 1800)]),
    (
        "long_slow_falling",
        "slow_falling",
        &[("slow_falling", 0, 4800)],
    ),
    ("wind_charged", "wind_charged", &[("wind_charged", 0, 3600)]),
    ("weaving", "weaving", &[("weaving", 0, 3600)]),
    ("oozing", "oozing", &[("oozing", 0, 3600)]),
    ("infested", "infested", &[("infested", 0, 3600)]),
];

/// The name suffixes the game's English names (`item.minecraft.potion.effect.*`).
const NAMED: [&str; 26] = [
    "water",
    "mundane",
    "thick",
    "awkward",
    "empty",
    "night_vision",
    "invisibility",
    "leaping",
    "fire_resistance",
    "swiftness",
    "slowness",
    "turtle_master",
    "water_breathing",
    "healing",
    "harming",
    "poison",
    "regeneration",
    "strength",
    "weakness",
    "luck",
    "slow_falling",
    "wind_charged",
    "weaving",
    "oozing",
    "infested",
    "levitation",
];

/// A vanilla id without its namespace.
fn short(id: &str) -> &str {
    id.strip_prefix("minecraft:").unwrap_or(id)
}

/// The registered potion `id` (with or without namespace) in [`POTIONS`].
pub fn registered(id: &str) -> Option<usize> {
    let id = short(id);
    POTIONS.iter().position(|(p, _, _)| *p == id)
}

/// `PotionContents.getColorOptional` of `(effect, amplifier)`s: their colours mixed, weighed by
/// amplifier + 1 (an effect the game doesn't know is left out); none without any.
fn mix<'a>(effects: impl Iterator<Item = (&'a str, i32)>) -> Option<u32> {
    let (mut r, mut g, mut b, mut total) = (0u32, 0u32, 0u32, 0u32);
    for (id, amp) in effects {
        let Some((_, color)) = EFFECTS.iter().find(|(e, _)| *e == short(id)) else {
            continue;
        };
        let w = amp.max(0) as u32 + 1;
        r += w * (color >> 16 & 255);
        g += w * (color >> 8 & 255);
        b += w * (color & 255);
        total += w;
    }
    (total > 0).then(|| ((r / total) << 16) | ((g / total) << 8) | (b / total))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok())
        .collect()
}

impl Contents {
    /// Contents of the registered potion `id` alone (`PotionContents(potion)`).
    pub fn of(id: &str) -> Contents {
        Contents {
            potion: Some(id.to_owned()),
            ..Contents::default()
        }
    }

    /// `PotionContents.EMPTY`: no potion, nothing custom.
    pub fn is_empty(&self) -> bool {
        *self == Contents::default()
    }

    /// Only a potion (or nothing): what a brewed or spout-filled bottle holds.
    pub fn is_plain(&self) -> bool {
        self.custom_color.is_none() && self.custom_name.is_none() && self.custom_effects.is_empty()
    }

    /// `PotionContents.is(Potions.WATER)` with no custom effects: a regular one is plain water
    /// (`PotionFluidHandler.getFluidFromPotionItem`).
    pub fn is_water(&self) -> bool {
        self.potion.as_deref().map(short) == Some("water") && self.custom_effects.is_empty()
    }

    /// `PotionContents.getColor`: the custom colour, else the mix of its potion's effects and
    /// its visible custom ones, else [`BASE_COLOR`]. 0xRRGGBB.
    pub fn color(&self) -> u32 {
        if let Some(c) = self.custom_color {
            return c as u32 & 0xFF_FFFF;
        }
        let potion = self
            .potion
            .as_deref()
            .and_then(registered)
            .map_or(&[][..], |i| POTIONS[i].2);
        let custom = self
            .effects
            .iter()
            .filter(|e| e.visible)
            .map(|e| (e.id.as_str(), e.amplifier));
        mix(potion.iter().map(|e| (e.0, e.1)).chain(custom)).unwrap_or(BASE_COLOR)
    }

    /// The suffix its name is made with (`PotionContents.getName`): the custom name, else its
    /// potion's (`Potion.name`), else `empty`.
    pub fn name_suffix(&self) -> &str {
        if let Some(n) = &self.custom_name {
            return n;
        }
        match self.potion.as_deref() {
            Some(p) => match registered(p) {
                Some(i) => POTIONS[i].1,
                None => p.rsplit(':').next().unwrap_or(p),
            },
            None => "empty",
        }
    }

    /// How it is saved and sent: a lone potion (or nothing) by its id; anything custom as
    /// `potion;colour;name;effects;read effects` (name and effects in hex, the read effects
    /// as `id,amplifier,visible` joined by `/`).
    pub fn encode(&self) -> String {
        let potion = self.potion.clone().unwrap_or_default();
        if self.is_plain() {
            return potion;
        }
        let color = self.custom_color.map(|c| c.to_string()).unwrap_or_default();
        let name = self.custom_name.as_deref().map(|n| hex(n.as_bytes()));
        let effects: Vec<String> = self
            .effects
            .iter()
            .map(|e| format!("{},{},{}", e.id, e.amplifier, u8::from(e.visible)))
            .collect();
        format!(
            "{potion};{color};{};{};{}",
            name.map_or_else(String::new, |n| format!("n{n}")),
            hex(&self.custom_effects),
            effects.join("/")
        )
    }

    /// Contents saved by [`Contents::encode`].
    pub fn decode(s: &str) -> Option<Contents> {
        let potion = |p: &str| (!p.is_empty()).then(|| p.to_owned());
        let parts: Vec<&str> = s.split(';').collect();
        let &[p, color, name, effects, read] = parts.as_slice() else {
            return (parts.len() == 1).then(|| Contents {
                potion: potion(s),
                ..Contents::default()
            });
        };
        let custom_name = match name.strip_prefix('n') {
            Some(n) => Some(String::from_utf8(unhex(n)?).ok()?),
            None => None,
        };
        let read = read
            .split('/')
            .filter(|e| !e.is_empty())
            .map(|e| {
                let mut f = e.split(',');
                Some(Effect {
                    id: f.next()?.to_owned(),
                    amplifier: f.next()?.parse().ok()?,
                    visible: f.next()? != "0",
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Contents {
            potion: potion(p),
            custom_color: if color.is_empty() {
                None
            } else {
                Some(color.parse().ok()?)
            },
            custom_name,
            custom_effects: unhex(effects)?,
            effects: read,
        })
    }
}

/// The English name of a bottle (`potion`, `splash_potion` or `lingering_potion`) with name
/// suffix `suffix` (`item.minecraft.<bottle>.effect.<suffix>`); the translation key for a
/// suffix the game has no name for.
pub fn english_name(bottle: &str, suffix: &str) -> String {
    let kind = match bottle {
        "splash_potion" => "Splash Potion",
        "lingering_potion" => "Lingering Potion",
        _ => "Potion",
    };
    if !NAMED.contains(&suffix) {
        return format!("item.minecraft.{bottle}.effect.{suffix}");
    }
    let title = |w: &str| {
        let mut c = w.chars();
        c.next()
            .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
            .unwrap_or_default()
    };
    let adjective = kind.trim_end_matches("Potion").trim_end();
    match suffix {
        "water" if adjective.is_empty() => "Water Bottle".into(),
        "water" => format!("{adjective} Water Bottle"),
        "empty" if adjective.is_empty() => "Uncraftable Potion".into(),
        "empty" => format!("{adjective} Uncraftable Potion"),
        "mundane" | "thick" | "awkward" => format!("{} {kind}", title(suffix)),
        s => {
            let of = match s {
                "turtle_master" => "the Turtle Master".to_owned(),
                "wind_charged" => "Wind Charging".to_owned(),
                "infested" => "Infestation".to_owned(),
                s => s.split('_').map(title).collect::<Vec<_>>().join(" "),
            };
            format!("{kind} of {of}")
        }
    }
}

/// The one shared copy of `c`.
pub fn intern(c: Contents) -> &'static Contents {
    static ALL: OnceLock<Mutex<HashSet<&'static Contents>>> = OnceLock::new();
    let mut all = ALL
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(c) = all.get(&c) {
        return c;
    }
    let c: &'static Contents = Box::leak(Box::new(c));
    all.insert(c);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_colour_as_the_game_mixes_them() {
        assert_eq!(Contents::of("minecraft:swiftness").color(), 0x33EBFF);
        // Slowness IV and resistance III: (0x8BAFE0 * 4 + 0x9146F0 * 3) / 7.
        assert_eq!(Contents::of("minecraft:turtle_master").color(), 0x8D82E6);
        assert_eq!(Contents::of("minecraft:awkward").color(), BASE_COLOR);
        assert_eq!(Contents::default().color(), BASE_COLOR);
        let custom = Contents {
            potion: Some("minecraft:water".into()),
            custom_effects: vec![1, 2, 3],
            effects: vec![
                Effect {
                    id: "minecraft:poison".into(),
                    amplifier: 0,
                    visible: true,
                },
                Effect {
                    id: "minecraft:speed".into(),
                    amplifier: 4,
                    visible: false,
                },
            ],
            ..Contents::default()
        };
        // The hidden one doesn't colour it.
        assert_eq!(custom.color(), 0x87A363);
        assert!(!custom.is_water());
        let coloured = Contents {
            custom_color: Some(0xFF12_3456_u32 as i32),
            ..custom.clone()
        };
        assert_eq!(coloured.color(), 0x123456);
    }

    #[test]
    fn contents_come_back_whole_from_their_encoding() {
        let plain = Contents::of("minecraft:long_swiftness");
        assert_eq!(plain.encode(), "minecraft:long_swiftness");
        assert_eq!(Contents::decode(&plain.encode()), Some(plain));
        assert_eq!(Contents::decode(""), Some(Contents::default()));
        let custom = Contents {
            potion: None,
            custom_color: Some(-5),
            custom_name: Some("my;odd|name#".into()),
            custom_effects: vec![9, 0, 255],
            effects: vec![Effect {
                id: "minecraft:haste".into(),
                amplifier: 2,
                visible: true,
            }],
        };
        let s = custom.encode();
        assert!(!s.contains('|') && !s.contains('#'));
        assert_eq!(Contents::decode(&s), Some(custom.clone()));
        assert!(std::ptr::eq(intern(custom.clone()), intern(custom)));
    }

    #[test]
    fn names_follow_the_game_s_english() {
        let name = |b, c: &Contents| english_name(b, c.name_suffix());
        assert_eq!(
            name("potion", &Contents::of("minecraft:strong_swiftness")),
            "Potion of Swiftness"
        );
        assert_eq!(
            name("splash_potion", &Contents::of("minecraft:water")),
            "Splash Water Bottle"
        );
        assert_eq!(
            name("lingering_potion", &Contents::default()),
            "Lingering Uncraftable Potion"
        );
        let levitation = Contents {
            custom_name: Some("levitation".into()),
            ..Contents::default()
        };
        assert_eq!(name("potion", &levitation), "Potion of Levitation");
        assert_eq!(
            english_name("potion", "mystery"),
            "item.minecraft.potion.effect.mystery"
        );
    }
}
