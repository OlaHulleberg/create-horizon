//! Create's funnels (`FunnelBlock`, `FunnelBlockEntity`) on an inventory: extracting, a stack
//! leaves the inventory behind every 8 ticks while nothing lies in front; inserting, items that
//! land on it go into the inventory.

use crate::kinetics::Dir;

/// `defaultExtractionTimer`.
pub const EXTRACTION_COOLDOWN: u32 = 8;
/// What an andesite funnel takes at once: a stack (no filter).
pub const AMOUNT: u32 = 64;

/// Where an extracted stack appears and how it moves (`activateExtractor`).
pub fn output(pos: [i32; 3], facing: Dir) -> ([f64; 3], [f64; 3]) {
    let o = facing.offset().map(f64::from);
    let vertical = facing.axis() == crate::kinetics::Axis::Y;
    let scale = match facing {
        Dir::Up => 0.15,
        Dir::Down => 0.5,
        _ => 0.25,
    };
    let mut at = [0, 1, 2].map(|a| f64::from(pos[a]) + 0.5 + o[a] * scale);
    if !vertical {
        at[1] -= 0.45;
    }
    let motion = if facing == Dir::Up {
        [0.0, 0.25, 0.0]
    } else {
        [0.0; 3]
    };
    (at, motion)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacks_come_out_in_front() {
        let (at, v) = output([0, 0, 0], Dir::East);
        assert_eq!(at, [0.75, 0.04999999999999999, 0.5]);
        assert_eq!(v, [0.0; 3]);
        assert_eq!(output([0, 0, 0], Dir::Up).1, [0.0, 0.25, 0.0]);
    }
}
