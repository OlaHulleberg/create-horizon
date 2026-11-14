//! `SequencedAssemblyItem`: a sequenced assembly's transitional item (an incomplete precision
//! mechanism, track or reinforced sheet) always shows a bar of its progress, from amber to
//! cyan (`getBarWidth`, `getBarColor`).

use super::custom;
use mod_api::{ClientHost, ItemBarEvent, ItemPart, Tag};

/// The transitional items, as `AllItems.sequencedIngredient` registers them.
pub(super) const IDS: [&str; 3] = [
    "create:incomplete_precision_mechanism",
    "create:unprocessed_obsidian_sheet",
    "create:incomplete_track",
];

pub(super) struct Transitional;

#[mod_api::horizon_part]
impl ItemPart for Transitional {
    fn item_bar(&self, _host: &dyn ClientHost, event: &mut ItemBarEvent<'_>) {
        let progress = progress(&custom(event.stack));
        *event.bar = Some((
            (progress * 13.0).round() as i32,
            mix(0xFFC074, 0x46FFE0, progress),
        ));
    }
}

/// `getProgress`: the `progress` of the stack's `create:sequenced_assembly` (0 without one).
fn progress(custom: &mod_api::Compound) -> f32 {
    let Some(Tag::Compound(c)) = custom.get(&"create:sequenced_assembly".into()) else {
        return 0.0;
    };
    match c.get(&"progress".into()) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// `Color.mixColors`: each channel of `a` moved `w` of the way to `b`'s.
fn mix(a: u32, b: u32, w: f32) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xFF) as f32, ((b >> shift) & 0xFF) as f32);
        ((x + (y - x) * w).round() as u32 & 0xFF) << shift
    };
    channel(16) | channel(8) | channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_goes_from_amber_to_cyan() {
        assert_eq!(mix(0xFFC074, 0x46FFE0, 0.0), 0xFFC074);
        assert_eq!(mix(0xFFC074, 0x46FFE0, 1.0), 0x46FFE0);
        let mut c = mod_api::Compound::new();
        assert_eq!(progress(&c), 0.0);
        let mut s = mod_api::Compound::new();
        s.insert("progress".into(), Tag::Float(0.5f32.to_bits()));
        c.insert("create:sequenced_assembly".into(), Tag::Compound(s));
        assert_eq!(progress(&c), 0.5);
    }
}
