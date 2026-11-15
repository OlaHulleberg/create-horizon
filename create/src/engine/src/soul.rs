//! The haunted bell's soul pulse (`SoulPulseEffect`, `HauntedBellPulser`, `SoulPulseEffectHandler`):
//! rings of blocks round a bell, one ring every 6 ticks outward, where a mob could spawn on the
//! ground; the glue spawns the soul particles where they are dark.

/// `SoulPulseEffect.MAX_DISTANCE`.
pub const MAX_DISTANCE: usize = 11;
/// `SoulPulseEffect.WAITING_TICKS`: a pulse lasts this long after its last ring.
pub const WAITING_TICKS: i32 = 100;
/// `SoulPulseEffect.TICKS_PER_LAYER`.
pub const TICKS_PER_LAYER: i32 = 6;
/// `HauntedBellPulser.DISTANCE`, `RECHARGE_TICKS`, `WARMUP_TICKS`: a held bell's pulse.
pub const HELD_DISTANCE: i32 = 3;
pub const HELD_RECHARGE: u64 = 8;
pub const HELD_WARMUP: i32 = 10;
/// `HauntedBellBlockEntity.DISTANCE` and `EFFECT_TICKS`: a rung bell's pulse and its souls.
pub const RUNG_DISTANCE: i32 = 10;
pub const EFFECT_TICKS: u32 = 20;

/// `SoulPulseEffect.genLayers`: the offsets at each rounded distance (1 to `MAX_DISTANCE`) from
/// the centre, in Create's order.
pub fn layers() -> Vec<Vec<[i32; 3]>> {
    let mut layers: Vec<Vec<[i32; 3]>> = vec![Vec::new(); MAX_DISTANCE];
    let m = MAX_DISTANCE as i32;
    for x in 0..m {
        for y in 0..m {
            for z in 0..m {
                let d = f64::from(x * x + y * y + z * z).sqrt().round() as i32;
                if d > m {
                    continue;
                }
                let d = d.max(1);
                let layer = &mut layers[(d - 1) as usize];
                let start = layer.len();
                let mut end = start + 1;
                layer.push([x, y, z]);
                if x != 0 {
                    layer.push([-x, y, z]);
                    end += 1;
                }
                if y != 0 {
                    for i in start..end {
                        let p = layer[i];
                        layer.push([p[0], -p[1], p[2]]);
                    }
                    end += end - start;
                }
                if z != 0 {
                    for i in start..end {
                        let p = layer[i];
                        layer.push([p[0], p[1], -p[2]]);
                    }
                }
            }
        }
    }
    layers
}

/// One pulse (`SoulPulseEffect`, not overlapping): its centre, distance and countdown, and the
/// blocks it took (`added`).
#[derive(Clone, Debug, PartialEq)]
pub struct Pulse {
    pub pos: [i32; 3],
    pub distance: i32,
    pub ticks: i32,
    pub added: Vec<[i32; 3]>,
}

impl Pulse {
    pub fn new(pos: [i32; 3], distance: i32) -> Pulse {
        Pulse {
            pos,
            distance,
            ticks: TICKS_PER_LAYER * distance,
            added: Vec::new(),
        }
    }

    /// `finished`.
    pub fn finished(&self) -> bool {
        self.ticks <= -WAITING_TICKS
    }

    fn layer_index(&self) -> i32 {
        self.distance - self.ticks / TICKS_PER_LAYER - 1
    }

    /// `tick`: the blocks of the ring it reaches this tick where `ok` (`canSpawnSoulAt`) lets a
    /// soul be, skipping on through empty rings; None between rings.
    pub fn tick(
        &mut self,
        layers: &[Vec<[i32; 3]>],
        mut ok: impl FnMut([i32; 3]) -> bool,
    ) -> Option<Vec<[i32; 3]>> {
        if self.finished() {
            return None;
        }
        self.ticks -= 1;
        if self.ticks < 0 || self.ticks % TICKS_PER_LAYER != 0 {
            return None;
        }
        let mut ring = |p: &Pulse| -> Vec<[i32; 3]> {
            let i = p.layer_index();
            if i < 0 || i as usize >= layers.len() {
                return Vec::new();
            }
            layers[i as usize]
                .iter()
                .map(|o| [p.pos[0] + o[0], p.pos[1] + o[1], p.pos[2] + o[2]])
                .filter(|q| ok(*q))
                .collect()
        };
        let mut spawns = ring(self);
        while spawns.is_empty() && self.ticks > 0 {
            self.ticks -= TICKS_PER_LAYER;
            spawns.extend(ring(self));
        }
        Some(spawns)
    }
}

/// `HauntedBellPulser.hauntedBellCreatesPulse`'s warm-up: `ticker` counts down from
/// `HELD_WARMUP` while the bell is held; whether a pulse goes out this tick (`game_time`).
pub fn held_pulse(ticker: &mut i32, game_time: u64) -> bool {
    let first = *ticker == 1;
    *ticker -= 1;
    if *ticker > 0 {
        return false;
    }
    first || !game_time.is_multiple_of(HELD_RECHARGE)
}

/// `SoulParticle`'s animation stages, as the particle's tick moves them: `stage` 1 start, 2 loop,
/// 3 end (0 when done), its ticks and loops so far, and the particle's draws (`startTicks`,
/// `endTicks`, `numLoops`). Returns the frame of the 53 to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoulAnimation {
    pub stage: u8,
    pub ticks: u32,
    pub loops: u32,
    pub start_ticks: u32,
    pub end_ticks: u32,
    pub num_loops: u32,
}

/// `SoulParticle.startFrames`, `loopFrames`, `endFrames`.
const START_FRAMES: u32 = 17;
const LOOP_FRAMES: u32 = 16;
const END_FRAMES: u32 = 20;

impl SoulAnimation {
    /// The stage's tick (`AnimationStage.tick`: a frame every 2 ticks), its frame, then the
    /// next stage (`getNext`).
    pub fn tick(&mut self) -> u32 {
        self.ticks += 1;
        let age = self.ticks / 2;
        let frame = match self.stage {
            1 => (age as f32 / self.start_ticks.max(1) as f32 * START_FRAMES as f32) as u32,
            2 => {
                let t = age % LOOP_FRAMES;
                if t == 0 {
                    self.loops += 1;
                }
                START_FRAMES + t
            }
            _ => {
                START_FRAMES
                    + LOOP_FRAMES
                    + (age as f32 / self.end_ticks.max(1) as f32 * END_FRAMES as f32) as u32
            }
        };
        let next = match self.stage {
            1 if age >= self.start_ticks => Some(2),
            2 if self.loops > self.num_loops => Some(3),
            3 if age >= self.end_ticks => Some(0),
            _ => None,
        };
        if let Some(n) = next {
            self.stage = n;
            self.ticks = 0;
            self.loops = 0;
        }
        // `setFrame` keeps the sprite for a frame past the 53.
        frame.min(START_FRAMES + LOOP_FRAMES + END_FRAMES - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_hold_each_offset_once_by_its_rounded_distance() {
        let l = layers();
        assert_eq!(l.len(), MAX_DISTANCE);
        // The centre and its six neighbours are at distance 1 (the centre's 0 rounds up).
        assert!(
            l[0].contains(&[0, 0, 0]) && l[0].contains(&[0, -1, 0]) && l[0].contains(&[1, 0, 0])
        );
        assert!(l[2].contains(&[-3, 0, 0]) && l[2].contains(&[2, 2, 0]));
        let all: usize = l.iter().map(Vec::len).sum();
        let mut seen = std::collections::HashSet::new();
        for layer in &l {
            for p in layer {
                assert!(seen.insert(*p), "{p:?} twice");
            }
        }
        assert_eq!(seen.len(), all);
    }

    #[test]
    fn a_pulse_rings_outward_every_six_ticks_then_waits() {
        let l = layers();
        let mut p = Pulse::new([0, 0, 0], 3);
        let mut rings = Vec::new();
        for t in 0..200 {
            if let Some(s) = p.tick(&l, |_| true) {
                rings.push((t, s.len()));
            }
        }
        // Rings 1, 2, 3 at the 6th, 12th and 18th ticks.
        assert_eq!(
            rings.iter().map(|r| r.0).collect::<Vec<_>>(),
            vec![5, 11, 17]
        );
        assert_eq!(rings[0].1, l[0].len());
        assert!(p.finished());
        // With nothing to show, the countdown runs through its rings at once.
        let mut q = Pulse::new([0, 0, 0], 3);
        let first = std::iter::repeat_with(|| q.tick(&l, |_| false)).find_map(|r| r);
        assert_eq!(first, Some(Vec::new()));
        assert_eq!(q.ticks, 0);
    }

    #[test]
    fn a_held_bell_warms_up_then_pulses_but_on_every_eighth_tick() {
        let mut t = HELD_WARMUP;
        let fired: Vec<bool> = (0..20u64).map(|g| held_pulse(&mut t, 80 + g)).collect();
        // Nine ticks of warm-up, the first pulse on the tenth (even on a multiple of 8).
        assert!(fired[..9].iter().all(|f| !f));
        assert!(fired[9]);
        assert!(!fired[16]);
        assert!(fired[10]);
    }

    #[test]
    fn a_soul_starts_loops_and_ends() {
        let mut a = SoulAnimation {
            stage: 1,
            ticks: 0,
            loops: 0,
            start_ticks: 17,
            end_ticks: 20,
            num_loops: 1,
        };
        let frames: Vec<u32> = std::iter::from_fn(|| (a.stage != 0).then(|| a.tick())).collect();
        assert_eq!(frames[0], 0);
        assert!(frames.contains(&17) && frames.contains(&32) && frames.contains(&33));
        assert!(*frames.iter().max().unwrap() < 53);
        assert_eq!(a.stage, 0);
    }
}
