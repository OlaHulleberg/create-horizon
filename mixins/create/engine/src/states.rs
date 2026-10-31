//! Which block states are Create's. Content-mod states are appended after vanilla's, one mod after
//! another in id order, so Create's states are one contiguous run of ids. Hot hooks test that run
//! with a single unsigned compare before doing anything else.

use std::sync::atomic::{AtomicU64, Ordering};

/// Create's state ids, `first..first + len`, packed in one word so a hook reads both with one load.
/// Zero length means no Create states are registered (the mod's content is off): every test misses.
static RANGE: AtomicU64 = AtomicU64::new(0);

/// Sets Create's state range for the world being opened.
pub fn set_range(first: u32, len: u32) {
    RANGE.store(u64::from(first) | (u64::from(len) << 32), Ordering::Relaxed);
}

/// Clears the range when the world closes.
pub fn clear_range() {
    RANGE.store(0, Ordering::Relaxed);
}

/// The index of `state` among Create's states, or `None` for any other block. This is the first
/// line of every hot hook: one load, one subtract, one compare; no hashing, strings or allocation.
#[inline(always)]
pub fn index(state: u32) -> Option<u32> {
    let range = RANGE.load(Ordering::Relaxed);
    let offset = state.wrapping_sub(range as u32);
    (offset < (range >> 32) as u32).then_some(offset)
}

/// Finds the contiguous run of states whose block name starts with `create:`.
/// Returns `(first, len)`, or `None` when there are none or they are not contiguous.
pub fn find_range<'a>(names: impl Iterator<Item = &'a str>) -> Option<(u32, u32)> {
    let mut first = None;
    let mut end = 0;
    for (i, name) in names.enumerate() {
        let i = i as u32;
        if name.starts_with("create:") {
            match first {
                None => first = Some(i),
                Some(_) if end != i => return None,
                Some(_) => {}
            }
            end = i + 1;
        }
    }
    first.map(|f| (f, end - f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_contiguous_run() {
        let names = [
            "minecraft:air",
            "minecraft:stone",
            "create:shaft",
            "create:shaft",
            "zz:x",
        ];
        assert_eq!(find_range(names.into_iter()), Some((2, 2)));
        assert_eq!(find_range(["minecraft:air"].into_iter()), None);
        assert_eq!(
            find_range(["create:a", "b:b", "create:c"].into_iter()),
            None
        );
    }

    #[test]
    fn index_misses_outside_the_range_and_when_cleared() {
        set_range(100, 3);
        assert_eq!(index(99), None);
        assert_eq!(index(100), Some(0));
        assert_eq!(index(102), Some(2));
        assert_eq!(index(103), None);
        assert_eq!(index(u32::MAX), None);
        clear_range();
        assert_eq!(index(100), None);
        assert_eq!(index(0), None);
    }
}
