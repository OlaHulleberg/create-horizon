//! Create's speedometer and stressometer (`GaugeBlock`, `GaugeBlockEntity`,
//! `SpeedGaugeBlockEntity`, `StressGaugeBlockEntity`, `GaugeRenderer`, `GaugeVisual`): a shaft
//! along their axis, and on their side faces a dial whose needle shows the speed or the network's
//! stress. The server works out the needle's target when a solve changes it; the client eases the
//! needle toward it and turns the dial on the GPU.

use crate::kinetics::{Axis, Dir, MAX_SPEED};
use crate::processing::Rng;

/// CKinetics `mediumSpeed` and `fastSpeed` (defaults).
pub const MEDIUM_SPEED: f32 = 30.0;
pub const FAST_SPEED: f32 = 100.0;

/// `GaugeBlock.Type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Speed,
    Stress,
}

/// `IRotate.SpeedLevel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpeedLevel {
    None,
    Slow,
    Medium,
    Fast,
}

impl SpeedLevel {
    /// `SpeedLevel.of`.
    pub fn of(speed: f32) -> SpeedLevel {
        let speed = speed.abs();
        if speed >= FAST_SPEED {
            SpeedLevel::Fast
        } else if speed >= MEDIUM_SPEED {
            SpeedLevel::Medium
        } else if speed >= 1.0 {
            SpeedLevel::Slow
        } else {
            SpeedLevel::None
        }
    }

    /// `SpeedLevel.getColor`.
    pub fn color(self) -> u32 {
        match self {
            SpeedLevel::None => 0x000000,
            SpeedLevel::Slow => 0x22FF22,
            SpeedLevel::Medium => 0x0084FF,
            SpeedLevel::Fast => 0xFF55FF,
        }
    }

    /// `SpeedLevel.getTextColor` (ChatFormatting DARK_GRAY, GREEN, AQUA, LIGHT_PURPLE).
    pub fn text_color(self) -> u32 {
        match self {
            SpeedLevel::None => 0x555555,
            SpeedLevel::Slow => 0x55FF55,
            SpeedLevel::Medium => 0x55FFFF,
            SpeedLevel::Fast => 0xFF55FF,
        }
    }

    /// `tooltip.speedRequirement.*` (en_us).
    pub fn name(self) -> &'static str {
        match self {
            SpeedLevel::None => "None",
            SpeedLevel::Slow => "Slow",
            SpeedLevel::Medium => "Moderate",
            SpeedLevel::Fast => "Fast",
        }
    }
}

/// `IRotate.StressImpact`, by a network's stress over its capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StressLevel {
    Low,
    Medium,
    High,
    Overstressed,
}

impl StressLevel {
    /// `StressImpact.of`.
    pub fn of(fraction: f64) -> StressLevel {
        if fraction > 1.0 {
            StressLevel::Overstressed
        } else if fraction > 0.75 {
            StressLevel::High
        } else if fraction > 0.5 {
            StressLevel::Medium
        } else {
            StressLevel::Low
        }
    }

    /// `StressImpact.getRelativeColor` (ChatFormatting GREEN, YELLOW, GOLD, RED).
    pub fn relative_color(self) -> u32 {
        match self {
            StressLevel::Low => 0x55FF55,
            StressLevel::Medium => 0xFFFF55,
            StressLevel::High => 0xFFAA00,
            StressLevel::Overstressed => 0xFF5555,
        }
    }

    /// `tooltip.stressImpact.*` (en_us).
    pub fn name(self) -> &'static str {
        match self {
            StressLevel::Low => "Low",
            StressLevel::Medium => "Moderate",
            StressLevel::High => "High",
            StressLevel::Overstressed => "Overstressed",
        }
    }
}

/// `Mth.lerp`.
fn lerp(delta: f32, start: f32, end: f32) -> f32 {
    start + delta * (end - start)
}

/// Catnip's `Color.mixColors`: each channel (alpha too) moved `w` of the way, truncated.
pub fn mix_colors(a: u32, b: u32, w: f32) -> u32 {
    let (a, b) = (a as i32, b as i32);
    let ch = |c: i32, shift: u32| (c >> shift) & 0xFF;
    let mix = |x: i32, y: i32| (x as f32 + (y - x) as f32 * w) as i32;
    let (a1, a2) = (a >> 24, b >> 24);
    ((mix(a1, a2) << 24)
        .wrapping_add(mix(ch(a, 16), ch(b, 16)) << 16)
        .wrapping_add(mix(ch(a, 8), ch(b, 8)) << 8)
        .wrapping_add(mix(ch(a, 0), ch(b, 0)))) as u32
}

/// `SpeedGaugeBlockEntity.getDialTarget`: 0 to .45 below medium speed, to .75 below fast, to
/// 1.125 at the maximum.
pub fn speed_dial(speed: f32) -> f32 {
    let speed = speed.abs();
    if speed == 0.0 {
        0.0
    } else if speed < MEDIUM_SPEED {
        lerp(speed / MEDIUM_SPEED, 0.0, 0.45)
    } else if speed < FAST_SPEED {
        lerp(
            (speed - MEDIUM_SPEED) / (FAST_SPEED - MEDIUM_SPEED),
            0.45,
            0.75,
        )
    } else {
        lerp((speed - FAST_SPEED) / (MAX_SPEED - FAST_SPEED), 0.75, 1.125)
    }
}

/// `SpeedGaugeBlockEntity.onSpeedChanged`: the speed level's colour, a quarter white.
pub fn speed_color(speed: f32) -> u32 {
    mix_colors(SpeedLevel::of(speed.abs()).color(), 0xFFFFFF, 0.25)
}

/// `StressGaugeBlockEntity.updateFromNetwork`: 1.125 overstressed, else the stress over the
/// capacity (0 without capacity). This port has no `disableStress`.
pub fn stress_dial(overstressed: bool, capacity: f32, stress: f32) -> f32 {
    if overstressed {
        1.125
    } else if capacity == 0.0 {
        0.0
    } else {
        stress / capacity
    }
}

/// `StressGaugeBlockEntity.updateFromNetwork`'s colour: green to yellow to red; kept as it was
/// while the dial is at 0.
pub fn stress_color(dial: f32, was: u32) -> u32 {
    if dial <= 0.0 {
        was
    } else if dial < 0.5 {
        mix_colors(0x00FF00, 0xFFFF00, dial * 2.0)
    } else if dial < 1.0 {
        mix_colors(0xFFFF00, 0xFF0000, dial * 2.0 - 1.0)
    } else {
        0xFF0000
    }
}

/// `GaugeBlock.getAnalogOutputSignal`: a comparator reads the dial target.
pub fn analog_output(dial: f32) -> i32 {
    (dial * 14.0).clamp(0.0, 15.0).ceil() as i32
}

/// What clients are told of a gauge: its dial target, and what its goggle tooltip reads (the
/// part's theoretical speed and its network's stress, capacity and overstress).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shown {
    pub dial: f32,
    pub speed: f32,
    pub stress: f32,
    pub capacity: f32,
    pub overstressed: bool,
}

/// A gauge on the server: its kind, what clients were last told and its colour
/// (`GaugeBlockEntity.dialTarget`, `color`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gauge {
    pub kind: Kind,
    pub shown: Shown,
    pub color: u32,
}

impl Gauge {
    pub fn new(kind: Kind) -> Gauge {
        Gauge {
            kind,
            shown: Shown::default(),
            color: 0,
        }
    }

    /// After a solve: `onSpeedChanged` and `updateFromNetwork` with the part's theoretical speed
    /// and its network's totals. A stressometer at rest (not overstressed) reads 0. True when
    /// clients must be told again.
    pub fn update(&mut self, speed: f32, stress: f32, capacity: f32, overstressed: bool) -> bool {
        let shown_speed = if overstressed { 0.0 } else { speed };
        let now = match self.kind {
            Kind::Speed => {
                self.color = speed_color(shown_speed);
                Shown {
                    dial: speed_dial(shown_speed),
                    speed,
                    stress: 0.0,
                    capacity: 0.0,
                    overstressed,
                }
            }
            Kind::Stress => {
                let dial = if shown_speed == 0.0 && !overstressed {
                    0.0
                } else {
                    stress_dial(overstressed, capacity, stress)
                };
                self.color = stress_color(dial, self.color);
                Shown {
                    dial,
                    speed,
                    stress,
                    capacity,
                    overstressed,
                }
            }
        };
        let changed = now != self.shown;
        self.shown = now;
        changed
    }
}

/// A needle on the client (`GaugeBlockEntity.dialState`, `prevDialState`), easing toward the
/// server's target.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Needle {
    pub target: f32,
    pub state: f32,
    pub prev: f32,
}

impl Needle {
    /// `GaugeBlockEntity.tick`: an eighth of the way each tick; past full scale it shakes back
    /// at random.
    pub fn tick(&mut self, rng: &mut Rng) {
        self.prev = self.state;
        self.state += (self.target - self.state) * 0.125;
        if self.state > 1.0 && rng.next_float() < 0.5 {
            self.state -= (self.state - 1.0) * rng.next_float();
        }
    }

    /// `Mth.equal(prevDialState, dialState)` (`GaugeVisual.beginFrame` skips it then), and no
    /// shaking left to do.
    pub fn settled(&self) -> bool {
        (self.state - self.prev).abs() < 1.0e-5 && self.state <= 1.0
    }
}

/// `GaugeBlock.shouldRenderHeadOnFace` from the state alone: a side face, not the back, not on
/// the shaft's axis, and only the front when the shaft is vertical; `Iterate.directions` order.
/// (Create also hides a face covered by its neighbour.)
pub fn dial_faces(facing: Dir, axis: Axis) -> impl Iterator<Item = Dir> {
    Dir::ALL.into_iter().filter(move |&f| {
        f.axis() != Axis::Y
            && f != facing.opposite()
            && f.axis() != axis
            && (axis != Axis::Y || f == facing)
    })
}

/// `GaugeRenderer`'s dial pivot: 5.75 px up and in from the model's corner.
pub const DIAL_PIVOT: f32 = 5.75 / 16.0;

/// The dial's model (`AllPartialModels.GAUGE_DIAL`), declared facing east (`rotateBufferTowards`
/// turns it by `-toYRot - 90`: 0° for east), turned to `face`: how far it is turned about the
/// block's centre on an axis, and how far it is moved (blocks), so that it turns about its pivot
/// as `rotate(-PI / 2 * progress, EAST)` between the pivot translations does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DialPose {
    pub axis: Axis,
    /// Degrees, counterclockwise looking down the positive axis.
    pub angle: f32,
    pub offset: [f32; 3],
}

/// `v` turned `deg` degrees about `axis`, counterclockwise looking down the positive axis.
fn turn(v: [f32; 3], axis: Axis, deg: f32) -> [f32; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    match axis {
        Axis::X => [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c],
        Axis::Y => [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c],
        Axis::Z => [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]],
    }
}

/// The turn about y from east to a side face (`-toYRot - 90`), degrees.
fn face_yaw(face: Dir) -> f32 {
    match face {
        Dir::South => -90.0,
        Dir::West => 180.0,
        Dir::North => 90.0,
        _ => 0.0,
    }
}

/// The dial on `face` at `progress` (`lerp(partialTicks, prevDialState, dialState)`).
pub fn dial_pose(face: Dir, progress: f32) -> DialPose {
    let yaw = face_yaw(face);
    // East's x axis turned to the face, and the pivot from the block's centre.
    let along = turn([1.0, 0.0, 0.0], Axis::Y, yaw);
    let pivot = turn([0.0, DIAL_PIVOT - 0.5, DIAL_PIVOT - 0.5], Axis::Y, yaw);
    let (axis, sign) = if along[0].abs() > 0.5 {
        (Axis::X, along[0].signum())
    } else {
        (Axis::Z, along[2].signum())
    };
    let angle = -90.0 * progress * sign;
    let turned = turn(pivot, axis, angle);
    DialPose {
        axis,
        angle,
        offset: [
            pivot[0] - turned[0],
            pivot[1] - turned[1],
            pivot[2] - turned[2],
        ],
    }
}

/// The dial on `face` gliding from `from` at `now` (seconds of the parts' clock) to `to` a tick
/// later, as a part's spin (rpm, phase at time 0) and motion (offset at `now`, velocity a
/// second): Create draws `lerp(partialTicks, prev, state)`. Its last glide is a still one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DialGlide {
    pub axis: Axis,
    pub rpm: f32,
    pub phase: f32,
    pub offset: [f32; 4],
    pub velocity: [f32; 3],
}

pub fn dial_glide(face: Dir, from: f32, to: f32, now: f32) -> DialGlide {
    let a = dial_pose(face, from);
    let b = dial_pose(face, to);
    // Degrees a second over the tick; the GPU's angle is `phase + rpm * 6 * seconds`.
    let rate = (b.angle - a.angle) * 20.0;
    DialGlide {
        axis: a.axis,
        rpm: rate / 6.0,
        phase: a.angle - rate * now,
        offset: [a.offset[0], a.offset[1], a.offset[2], now],
        velocity: [
            (b.offset[0] - a.offset[0]) * 20.0,
            (b.offset[1] - a.offset[1]) * 20.0,
            (b.offset[2] - a.offset[2]) * 20.0,
        ],
    }
}

/// `CreateLang.number` (LangNumberFormat, en_us): grouped thousands, at most two decimals.
pub fn number(v: f64) -> String {
    let text = format!("{:.2}", v);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    let (sign, text) = match text.strip_prefix('-') {
        Some(t) => ("-", t),
        None => ("", text),
    };
    let (int, frac) = text.split_once('.').unwrap_or((text, ""));
    let mut grouped = String::from(sign);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if !frac.is_empty() {
        grouped.push('.');
        grouped.push_str(frac);
    }
    grouped
}

/// `TooltipHelper.makeProgressBar`.
pub fn progress_bar(length: usize, filled: usize) -> String {
    let mut bar = String::from(" ");
    for _ in 0..filled {
        bar.push('\u{2588}');
    }
    for _ in filled..length {
        bar.push('\u{2592}');
    }
    bar.push(' ');
    bar
}

/// One tooltip line: its pieces and their colours.
pub type Line = Vec<(String, u32)>;

/// ChatFormatting WHITE, GRAY and DARK_GRAY.
const WHITE: u32 = 0xFFFFFF;
const GRAY: u32 = 0xAAAAAA;
const DARK_GRAY: u32 = 0x555555;

/// `forGoggles`: four spaces, and one more per indent.
fn goggles(indent: usize, pieces: Line) -> Line {
    let mut line = vec![(" ".repeat(4 + indent), WHITE)];
    line.extend(pieces);
    line
}

/// `SpeedGaugeBlockEntity.addToGoggleTooltip` and `StressGaugeBlockEntity.addToGoggleTooltip`
/// (en_us). An overstressed speed is struck through in Create; here it is only dark grey.
pub fn tooltip(kind: Kind, s: &Shown) -> Vec<Line> {
    let mut lines = vec![goggles(0, vec![("Gauge Information:".into(), WHITE)])];
    match kind {
        Kind::Speed => {
            lines.push(goggles(0, vec![("Rotation Speed".into(), GRAY)]));
            // `SpeedLevel.getFormattedSpeedText(speed, isOverStressed())`.
            let level = SpeedLevel::of(s.speed);
            let text = format!(
                "{}{} ({} RPM) ",
                progress_bar(3, level as usize),
                level.name(),
                number(f64::from(s.speed.abs()))
            );
            let color = if s.overstressed {
                DARK_GRAY
            } else {
                level.text_color()
            };
            lines.push(goggles(0, vec![(text, color)]));
        }
        Kind::Stress => {
            let capacity = f64::from(s.capacity);
            let stress = f64::from(s.stress);
            let fraction = stress / if capacity == 0.0 { 1.0 } else { capacity };
            lines.push(goggles(0, vec![("Network Stress".into(), GRAY)]));
            if s.speed == 0.0 {
                lines.push(goggles(
                    0,
                    vec![(format!("{}No Rotation", progress_bar(3, 0)), DARK_GRAY)],
                ));
            } else {
                // `StressImpact.getFormattedStressText`.
                let level = StressLevel::of(fraction);
                lines.push(goggles(
                    0,
                    vec![(
                        format!(
                            "{}{} ({}%) ",
                            progress_bar(3, (level as usize + 1).min(3)),
                            level.name(),
                            (fraction * 100.0) as i32
                        ),
                        level.relative_color(),
                    )],
                ));
                lines.push(goggles(0, vec![("Remaining Capacity".into(), GRAY)]));
                let remaining = capacity - stress;
                let mut tip = vec![(format!("{}su", number(remaining)), level.relative_color())];
                if remaining != capacity {
                    tip.push((" / ".into(), GRAY));
                    tip.push((format!("{}su", number(capacity)), DARK_GRAY));
                }
                lines.push(goggles(1, tip));
            }
        }
    }
    lines
}

/// `GaugeBlock.getStateForPlacement`: on a block with a shaft toward the clicked face, the shaft
/// runs on through the gauge (its facing from the look, its axis that face's); otherwise
/// `DirectionalAxisKineticBlock`'s, facing the clicked face. `look` is the nearest looking
/// direction, `horizontal` the player's facing; `shaft(side)` whether the block at `side` has a
/// shaft facing back. The facing and `axis_along_first`.
pub fn placement(
    clicked_face: Dir,
    look: Dir,
    horizontal: Dir,
    placed_on_shaft: bool,
    shaft: impl Fn(Dir) -> bool,
) -> (Dir, bool) {
    if placed_on_shaft {
        let look_positive = look.positive();
        return match clicked_face.axis() {
            Axis::X => (
                if look_positive {
                    Dir::North
                } else {
                    Dir::South
                },
                true,
            ),
            Axis::Y => (horizontal.opposite(), horizontal.axis() == Axis::X),
            Axis::Z => (if look_positive { Dir::West } else { Dir::East }, false),
        };
    }
    // DirectionalAxisKineticBlock.getStateForPlacement with GaugeBlock's `getFacingForPlacement`
    // (the clicked face) and `getAxisAlignmentForPlacement` (the player's facing not on x).
    let facing = clicked_face;
    let along_first = match facing.axis() {
        Axis::Y => {
            let mut preferred: Option<Dir> = None;
            for side in [Dir::North, Dir::South, Dir::West, Dir::East] {
                if !shaft(side) {
                    continue;
                }
                if preferred.is_some_and(|p| p.axis() != side.axis()) {
                    preferred = None;
                    break;
                }
                preferred = Some(side);
            }
            preferred.map_or(horizontal.axis() != Axis::X, |p| p.axis() == Axis::X)
        }
        // `prefersConnectionTo` with `shaftAxis` false is always false: only above and below.
        a => {
            if shaft(Dir::Up) || shaft(Dir::Down) {
                a == Axis::X
            } else {
                a == Axis::Z
            }
        }
    };
    (facing, along_first)
}

/// The channel the server sends gauges on.
pub const CHANNEL: &str = "create:gauge";

/// Bytes of one record: position, dial, speed, stress, capacity, overstressed.
const RECORD: usize = 12 + 4 * 4 + 1;

/// Gauges as messages of at most 30,000 bytes each.
pub fn encode(all: &[([i32; 3], Shown)]) -> Vec<Vec<u8>> {
    all.chunks(30_000 / RECORD)
        .map(|chunk| {
            let mut out = Vec::with_capacity(chunk.len() * RECORD);
            for (pos, s) in chunk {
                for v in pos {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                for v in [s.dial, s.speed, s.stress, s.capacity] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                out.push(u8::from(s.overstressed));
            }
            out
        })
        .collect()
}

/// A message's gauges; a malformed tail is dropped.
pub fn decode(data: &[u8]) -> Vec<([i32; 3], Shown)> {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    let f = |b: &[u8]| f32::from_le_bytes(b.try_into().unwrap());
    data.chunks_exact(RECORD)
        .map(|r| {
            (
                [i(&r[0..4]), i(&r[4..8]), i(&r[8..12])],
                Shown {
                    dial: f(&r[12..16]),
                    speed: f(&r[16..20]),
                    stress: f(&r[20..24]),
                    capacity: f(&r[24..28]),
                    overstressed: r[28] != 0,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn the_speed_dial_follows_create_s_three_ramps() {
        assert_eq!(speed_dial(0.0), 0.0);
        assert!(close(speed_dial(15.0), 0.225));
        assert!(close(speed_dial(-30.0), 0.45));
        assert!(close(speed_dial(65.0), 0.6));
        assert!(close(speed_dial(100.0), 0.75));
        assert!(close(speed_dial(256.0), 1.125));
        assert!(close(speed_dial(178.0), 0.9375));
    }

    #[test]
    fn speed_colours_are_a_quarter_white() {
        assert_eq!(speed_color(0.0), 0x3F3F3F);
        assert_eq!(speed_color(16.0), 0x59FF59);
        assert_eq!(speed_color(-32.0), 0x3FA2FF);
        assert_eq!(speed_color(128.0), 0xFF7FFF);
        assert_eq!(SpeedLevel::of(0.5), SpeedLevel::None);
        assert_eq!(SpeedLevel::of(1.0), SpeedLevel::Slow);
    }

    #[test]
    fn the_stress_dial_is_stress_over_capacity() {
        assert_eq!(stress_dial(false, 0.0, 10.0), 0.0);
        assert_eq!(stress_dial(false, 256.0, 64.0), 0.25);
        assert_eq!(stress_dial(true, 256.0, 512.0), 1.125);
        assert_eq!(stress_color(0.25, 7), 0x7FFF00);
        assert_eq!(stress_color(0.75, 7), 0xFF7F00);
        assert_eq!(stress_color(1.125, 7), 0xFF0000);
        assert_eq!(stress_color(0.0, 7), 7);
    }

    #[test]
    fn comparators_read_the_dial_in_fourteenths() {
        assert_eq!(analog_output(0.0), 0);
        assert_eq!(analog_output(0.01), 1);
        assert_eq!(analog_output(0.5), 7);
        assert_eq!(analog_output(1.0), 14);
        assert_eq!(analog_output(1.125), 15);
    }

    #[test]
    fn gauges_tell_clients_only_of_changes() {
        let mut g = Gauge::new(Kind::Speed);
        assert!(!g.update(0.0, 0.0, 0.0, false));
        assert!(g.update(64.0, 128.0, 1024.0, false));
        assert!(
            !g.update(64.0, 256.0, 1024.0, false),
            "speed reads no stress"
        );
        // Overstressed, a speedometer shows no speed (`getSpeed` is 0).
        assert!(g.update(64.0, 2048.0, 1024.0, true));
        assert_eq!(g.shown.dial, 0.0);

        let mut s = Gauge::new(Kind::Stress);
        assert!(s.update(16.0, 64.0, 256.0, false));
        assert_eq!(s.shown.dial, 0.25);
        assert!(s.update(16.0, 512.0, 256.0, true));
        assert_eq!(s.shown.dial, 1.125);
        assert_eq!(s.color, 0xFF0000);
        // Stopped: 0, the colour kept.
        assert!(s.update(0.0, 0.0, 0.0, false));
        assert_eq!((s.shown.dial, s.color), (0.0, 0xFF0000));
    }

    #[test]
    fn needles_ease_an_eighth_a_tick_and_shake_past_full_scale() {
        let mut rng = Rng::new(7);
        let mut n = Needle {
            target: 0.5,
            ..Needle::default()
        };
        n.tick(&mut rng);
        assert!(close(n.state, 0.0625));
        for _ in 0..200 {
            n.tick(&mut rng);
        }
        assert!(n.settled());
        n.target = 1.125;
        for _ in 0..200 {
            n.tick(&mut rng);
            assert!(n.state <= 1.125);
        }
        assert!(!n.settled());
    }

    #[test]
    fn dials_show_on_the_side_faces() {
        // Facing up, shaft along x: north and south.
        let f: Vec<Dir> = dial_faces(Dir::Up, Axis::X).collect();
        assert_eq!(f, [Dir::North, Dir::South]);
        // Facing north on a vertical shaft: the front only.
        let f: Vec<Dir> = dial_faces(Dir::North, Axis::Y).collect();
        assert_eq!(f, [Dir::North]);
        // Facing north, shaft along x: the front (the back is the opposite face).
        let f: Vec<Dir> = dial_faces(Dir::North, Axis::X).collect();
        assert_eq!(f, [Dir::North]);
    }

    #[test]
    fn dials_turn_about_their_pivot() {
        let a = 0.5 - DIAL_PIVOT;
        let rest = dial_pose(Dir::East, 0.0);
        assert_eq!(rest.axis, Axis::X);
        assert!(rest.angle == 0.0 && rest.offset.iter().all(|v| v.abs() < 1e-6));
        // East at full scale: -90° about x moves the pivot's corner by 2a toward north.
        let full = dial_pose(Dir::East, 1.0);
        assert_eq!(full.angle, -90.0);
        assert!(close(full.offset[0], 0.0) && close(full.offset[1], 0.0));
        assert!(close(full.offset[2], -2.0 * a));
        // North's x axis is east's turned to -z: about z the other way.
        let north = dial_pose(Dir::North, 1.0);
        assert_eq!((north.axis, north.angle), (Axis::Z, 90.0));
        // On every face the pivot stays where it is.
        for face in [Dir::North, Dir::South, Dir::West, Dir::East] {
            let yaw = face_yaw(face);
            let pivot = turn([0.0, -a, -a], Axis::Y, yaw);
            for p in [0.3, 0.75, 1.125] {
                let d = dial_pose(face, p);
                let moved = turn(pivot, d.axis, d.angle);
                for i in 0..3 {
                    assert!(close(moved[i] + d.offset[i], pivot[i]), "{face:?} {p}");
                }
            }
        }
    }

    #[test]
    fn a_glide_starts_where_the_needle_was_and_ends_a_tick_on() {
        let now = 12.5;
        let g = dial_glide(Dir::West, 0.25, 0.5, now);
        let (a, b) = (dial_pose(Dir::West, 0.25), dial_pose(Dir::West, 0.5));
        assert!(close(g.phase + g.rpm * 6.0 * now, a.angle));
        assert!(close(g.phase + g.rpm * 6.0 * (now + 0.05), b.angle));
        for i in 0..3 {
            assert!(close(g.offset[i] + g.velocity[i] * 0.05, b.offset[i]));
        }
        // At rest: no spin, no motion.
        let still = dial_glide(Dir::West, 0.5, 0.5, now);
        assert_eq!((still.rpm, still.velocity), (0.0, [0.0; 3]));
    }

    #[test]
    fn tooltips_read_as_create_s() {
        let s = Shown {
            dial: 0.6,
            speed: -64.0,
            ..Shown::default()
        };
        let t = tooltip(Kind::Speed, &s);
        assert_eq!(t[0][1].0, "Gauge Information:");
        assert_eq!(
            t[2][1],
            (
                " \u{2588}\u{2588}\u{2592} Moderate (64 RPM) ".into(),
                0x55FFFF
            )
        );
        let s = Shown {
            dial: 0.25,
            speed: 16.0,
            stress: 4096.0,
            capacity: 16384.0,
            overstressed: false,
        };
        let t = tooltip(Kind::Stress, &s);
        assert_eq!(t[2][1].0, " \u{2588}\u{2592}\u{2592} Low (25%) ");
        assert_eq!(t[4][0].0, "     ");
        assert_eq!(t[4][1].0, "12,288su");
        assert_eq!(t[4][3], ("16,384su".into(), 0x555555));
        let still = tooltip(Kind::Stress, &Shown::default());
        assert_eq!(still[2][1].0, " \u{2592}\u{2592}\u{2592} No Rotation");
        assert_eq!(number(1234567.5), "1,234,567.5");
        assert_eq!(number(-1000.0), "-1,000");
    }

    #[test]
    fn placed_on_a_shaft_the_shaft_runs_through() {
        let none = |_: Dir| false;
        // On an x shaft's east end, looking west (negative): facing south, along x.
        assert_eq!(
            placement(Dir::East, Dir::West, Dir::West, true, none),
            (Dir::South, true)
        );
        // On top of a vertical shaft, facing east: west, along x (its axis y).
        assert_eq!(
            placement(Dir::Up, Dir::Down, Dir::East, true, none),
            (Dir::West, true)
        );
        assert_eq!(
            placement(Dir::South, Dir::South, Dir::South, true, none),
            (Dir::West, false)
        );
        // Elsewhere it faces the clicked face; a shaft above turns a north gauge's axis to y.
        assert_eq!(
            placement(Dir::North, Dir::South, Dir::South, false, none),
            (Dir::North, true)
        );
        assert_eq!(
            placement(Dir::North, Dir::South, Dir::South, false, |d| d == Dir::Up),
            (Dir::North, false)
        );
        // On a floor, the player's facing; a shaft to the west sets x.
        assert_eq!(
            placement(Dir::Up, Dir::Down, Dir::East, false, none),
            (Dir::Up, false)
        );
        assert_eq!(
            placement(Dir::Up, Dir::Down, Dir::North, false, |d| d == Dir::West),
            (Dir::Up, true)
        );
    }

    #[test]
    fn messages_round_trip() {
        let all: Vec<([i32; 3], Shown)> = (0..2000)
            .map(|i| {
                (
                    [i, -64, -i],
                    Shown {
                        dial: i as f32 / 2000.0,
                        speed: -16.0,
                        stress: 8.0,
                        capacity: 256.0,
                        overstressed: i % 2 == 0,
                    },
                )
            })
            .collect();
        let m = encode(&all);
        assert!(m.len() > 1 && m.iter().all(|m| m.len() <= 30_000));
        let back: Vec<_> = m.iter().flat_map(|m| decode(m)).collect();
        assert_eq!(back, all);
    }
}
