//! Create's steam whistle (`WhistleBlock`, `WhistleBlockEntity`, `WhistleExtenderBlock`,
//! `WhistleSoundInstance`): a whistle on a fluid tank makes it a boiler (`BoilerData`'s attached
//! whistles); powered by redstone while that boiler is heated, it sounds, its pitch set by its
//! size (the octave) and the extensions stacked on it. It consumes nothing.

use crate::kinetics::Dir;

/// `WhistleBlock.SIZE`, in its order (`cycle` goes small, medium, large, small).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Size {
    Small,
    #[default]
    Medium,
    Large,
}

impl Size {
    pub fn read(v: &str) -> Option<Size> {
        Some(match v {
            "small" => Size::Small,
            "medium" => Size::Medium,
            "large" => Size::Large,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Medium => "medium",
            Size::Large => "large",
        }
    }
    /// `BlockState.cycle(SIZE)` (`getRotatedBlockState`: the wrench).
    pub fn cycled(self) -> Size {
        match self {
            Size::Small => Size::Medium,
            Size::Medium => Size::Large,
            Size::Large => Size::Small,
        }
    }
    /// `WhistleSoundInstance`'s sound: high, medium or low whistling (`AllSoundEvents
    /// .WHISTLE_HIGH`, `WHISTLE_MEDIUM`, `WHISTLE_LOW`).
    pub fn sound(self) -> &'static str {
        match self {
            Size::Small => "create:whistle_high",
            Size::Medium => "create:whistle",
            Size::Large => "create:whistle_low",
        }
    }
}

/// `WhistleExtenderBlock.SHAPE`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    #[default]
    Single,
    Double,
    DoubleConnected,
}

impl Shape {
    pub fn read(v: &str) -> Option<Shape> {
        Some(match v {
            "single" => Shape::Single,
            "double" => Shape::Double,
            "double_connected" => Shape::DoubleConnected,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Shape::Single => "single",
            Shape::Double => "double",
            Shape::DoubleConnected => "double_connected",
        }
    }
}

/// `WhistleBlock.getAttachedDirection`: its facing on a wall, down on the floor.
pub fn attached(wall: bool, facing: Dir) -> Dir {
    if wall { facing } else { Dir::Down }
}

/// `WhistleBlock.getStateForPlacement`: on a side of the block clicked, facing back into it
/// (`wall`); on its top or bottom, facing the way the player looks (on the floor). `(wall,
/// facing)`; its tank must be the way it is attached (`canSurvive`).
pub fn placement(clicked: Dir, horizontal: Dir) -> (bool, Dir) {
    if clicked.axis() == crate::kinetics::Axis::Y {
        (false, horizontal)
    } else {
        (true, clicked.opposite())
    }
}

/// `WhistleBlockEntity.updatePitch`: two steps per doubled extension above it, one more for a
/// single one at the top (which ends it).
pub fn pitch(above: impl Fn(i32) -> Option<Shape>) -> i32 {
    let mut pitch = 0;
    let mut height = 1;
    while pitch <= 24 {
        match above(height) {
            None => break,
            Some(Shape::Single) => {
                pitch += 1;
                break;
            }
            Some(_) => {}
        }
        height += 1;
        pitch += 2;
    }
    pitch
}

/// `tickAudio`: the sound's pitch, `2^(-pitch / 12)`.
pub fn sound_pitch(pitch: i32) -> f32 {
    2f32.powf(-(pitch as f32) / 12.0)
}

/// The chiff a whistle starts with (`AllSoundEvents.WHISTLE_CHIFF`): its pitch (`f + .75` for a
/// small one).
pub fn chiff_pitch(size: Size, pitch: i32) -> f32 {
    let f = sound_pitch(pitch);
    if size == Size::Small { f + 0.75 } else { f }
}

/// `WhistleBlockEntity.tick`'s `powered`: redstone, and a boiler that is active and heated
/// (passively, or by a burner).
pub fn sounds(powered: bool, boiler: Option<&crate::steam::Boiler>) -> bool {
    powered && boiler.is_some_and(|b| b.is_active() && (b.passive_heat || b.active_heat > 0))
}

/// What `incrementSize` does going up from the whistle: the first single extension becomes a
/// double; else the first block that is no extension takes a new single one, if it can be
/// replaced; at most 6 blocks up. The height and the pitch of the sounds it makes (the
/// xylophone note and the block's hit sound).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grow {
    Double(i32, f32),
    Place(i32, f32),
}

pub fn grow(
    above: impl Fn(i32) -> Option<Shape>,
    replaceable: impl Fn(i32) -> bool,
) -> Option<Grow> {
    for i in 1..=6 {
        match above(i) {
            Some(Shape::Single) => {
                return Some(Grow::Double(i, 2f32.powf(-(i as f32 * 2.0) / 12.0)));
            }
            Some(_) => continue,
            None => {
                if !replaceable(i) {
                    return None;
                }
                return Some(Grow::Place(i, 2f32.powf(-(i as f32 * 2.0 - 1.0) / 12.0)));
            }
        }
    }
    None
}

/// `WhistleExtenderBlock.updateShape` from above: a double extension joins the one above it
/// (`double_connected`) and leaves it when it goes.
pub fn connected(shape: Shape, extension_above: bool) -> Shape {
    match (shape, extension_above) {
        (Shape::Double, true) => Shape::DoubleConnected,
        (Shape::DoubleConnected, false) => Shape::Double,
        (s, _) => s,
    }
}

/// `WhistleExtenderBlock.canSurvive`: on a whistle, or on a doubled extension.
pub fn extension_survives(below_whistle: bool, below_extension: Option<Shape>) -> bool {
    below_whistle || below_extension.is_some_and(|s| s != Shape::Single)
}

/// How long a whistle's sound plays, in ticks, before it is started again: its 3 s loop at
/// `pitch` (Minecraft clamps a sound's pitch to 0.5–2), less a tick so it doesn't fall silent.
pub fn replay_ticks(pitch: i32) -> u32 {
    let f = sound_pitch(pitch).clamp(0.5, 2.0);
    ((60.0 / f) as u32).saturating_sub(1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_set_the_pitch_in_semitones() {
        // No extension: 0. One single: 1. Two doubles and a single: 5.
        assert_eq!(pitch(|_| None), 0);
        assert_eq!(pitch(|h| (h == 1).then_some(Shape::Single)), 1);
        let stack = |h: i32| match h {
            1 => Some(Shape::DoubleConnected),
            2 => Some(Shape::Double),
            3 => Some(Shape::Single),
            _ => None,
        };
        assert_eq!(pitch(stack), 5);
        assert_eq!(sound_pitch(0), 1.0);
        assert!((sound_pitch(12) - 0.5).abs() < 1e-6);
        assert!((chiff_pitch(Size::Small, 0) - 1.75).abs() < 1e-6);
        assert_eq!(Size::Small.sound(), "create:whistle_high");
        assert_eq!(Size::Large.cycled(), Size::Small);
        assert_eq!(replay_ticks(0), 59);
        assert_eq!(replay_ticks(24), 119);
    }

    #[test]
    fn a_whistle_grows_a_semitone_at_a_time() {
        // Nothing above: a single extension one up, 2^(-1/12).
        let g = grow(|_| None, |_| true);
        assert!(matches!(g, Some(Grow::Place(1, p)) if (p - 2f32.powf(-1.0 / 12.0)).abs() < 1e-6));
        // A single one above: it doubles.
        assert!(matches!(
            grow(|h| (h == 1).then_some(Shape::Single), |_| true),
            Some(Grow::Double(1, _))
        ));
        // A double one, then a solid block: nothing.
        assert_eq!(grow(|h| (h == 1).then_some(Shape::Double), |_| false), None);
        // Six doubles: as tall as it goes.
        assert_eq!(grow(|_| Some(Shape::Double), |_| true), None);
        assert_eq!(connected(Shape::Double, true), Shape::DoubleConnected);
        assert_eq!(connected(Shape::DoubleConnected, false), Shape::Double);
        assert!(extension_survives(false, Some(Shape::Double)));
        assert!(!extension_survives(false, Some(Shape::Single)));
    }

    #[test]
    fn it_sounds_only_powered_on_a_heated_boiler() {
        use crate::steam::Boiler;
        let mut b = Boiler {
            attached_whistles: 1,
            ..Boiler::default()
        };
        assert!(!sounds(true, Some(&b)), "no heat");
        b.passive_heat = true;
        assert!(sounds(true, Some(&b)));
        assert!(!sounds(false, Some(&b)));
        assert!(!sounds(true, None));
        assert_eq!(placement(Dir::East, Dir::North), (true, Dir::West));
        assert_eq!(placement(Dir::Up, Dir::North), (false, Dir::North));
        assert_eq!(attached(false, Dir::North), Dir::Down);
    }
}
