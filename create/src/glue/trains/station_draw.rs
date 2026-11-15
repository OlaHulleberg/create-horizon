//! What a station draws besides its block (`StationRenderer`): its flag, raised while a train
//! stands at it or it is in assembly mode (`flag_on`, `flag_off`, `flag_assemble`, turned to the
//! track and swung up as `transformFlag` does), the station marker on its track
//! (`TrackTargetingBehaviour.render` of `track_overlay/station`), and in assembly mode the
//! assembly track's `track_overlay/assembling` where a bogey may go and where one is, blue and
//! green (a copy of the model with every face tinted, made by `create-data prepare`).
//!
//! The server keeps what clients need in the station's block entity data (`KEY`), refreshed
//! every ten ticks and when its train comes or goes. The station keeps no facing along its track
//! here (Create's `TrackTargetingBehaviour.targetDirection`, set from the placing player's
//! look): the flag is turned by the assembly direction last used, else the track's positive way.
//! The flag's sounds (`CONTRAPTION_ASSEMBLE` / `DISASSEMBLE` as it moves) are not played: the
//! drawing has no host to play them from.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::track::TrackShape;
use mod_api::{BlockEntityDrawContext, ClientHost, Compound, Tag};
use std::cell::RefCell;
use std::collections::HashMap;

/// The block entity data key.
pub const KEY: &str = "create_station";

/// The standalone models drawn.
pub const FLAG_ON: &str = "create:block/track_station/flag_on";
pub const FLAG_OFF: &str = "create:block/track_station/flag_off";
pub const FLAG_ASSEMBLE: &str = "create:block/track_station/flag_assemble";
pub const OVERLAY: &str = "create:block/track_overlay/station";
/// The assembly overlay with every face tinted (made by `create-data prepare`), coloured as
/// Create's `SuperByteBuffer.color` colours it.
pub const ASSEMBLING: &str = "create:block/track_overlay/assembling_tinted";
pub const SIGNAL: &str = "create:block/track_overlay/signal";
pub const SIGNAL_DUAL: &str = "create:block/track_overlay/signal_dual";
pub const OBSERVER: &str = "create:block/track_overlay/observer";
pub const MODELS: [&str; 8] = [
    FLAG_ON,
    FLAG_OFF,
    FLAG_ASSEMBLE,
    OVERLAY,
    ASSEMBLING,
    SIGNAL,
    SIGNAL_DUAL,
    OBSERVER,
];

/// The block entity data key of a signal's or track observer's marker on its track: its track
/// block from it, the way it looks, the marker (0 signal, 1 dual signal, 2 observer) and the
/// light it is drawn with.
pub const MARK: &str = "TrackMark";

/// What a station shows, as its data carries it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Shown {
    /// Its track block from the station, and the unit along it the station looks.
    track: [i32; 3],
    dir: [i32; 2],
    present: bool,
    assembling: bool,
    /// `assemblyLength` and `bogeyLocations`.
    length: i32,
    bogeys: Vec<i32>,
    /// The light its flag and its track are drawn with, read on the server (a client's level
    /// light can't be read; the station's own cell is dark inside its block): packed as the
    /// draw context takes it.
    light: [i32; 2],
}

impl Shown {
    fn to_nbt(&self) -> Compound {
        Compound::from([
            ("t".into(), Tag::IntArray(self.track.to_vec())),
            ("d".into(), Tag::IntArray(self.dir.to_vec())),
            ("p".into(), Tag::Byte(i8::from(self.present))),
            ("a".into(), Tag::Byte(i8::from(self.assembling))),
            ("n".into(), Tag::Int(self.length)),
            ("b".into(), Tag::IntArray(self.bogeys.clone())),
            ("l".into(), Tag::IntArray(self.light.to_vec())),
        ])
    }

    fn of(c: &Compound) -> Option<Shown> {
        let ints = |k: &str| match c.get(&k.into()) {
            Some(Tag::IntArray(v)) => Some(v.clone()),
            _ => None,
        };
        let flag = |k: &str| matches!(c.get(&k.into()), Some(Tag::Byte(1)));
        let t = ints("t")?;
        let d = ints("d")?;
        Some(Shown {
            track: [*t.first()?, *t.get(1)?, *t.get(2)?],
            dir: [*d.first()?, *d.get(1)?],
            present: flag("p"),
            assembling: flag("a"),
            length: match c.get(&"n".into()) {
                Some(Tag::Int(n)) => *n,
                _ => 0,
            },
            bogeys: ints("b").unwrap_or_default(),
            light: ints("l")
                .and_then(|l| Some([*l.first()?, *l.get(1)?]))
                .unwrap_or([0xF0, 0xF0]),
        })
    }
}

/// A station by level and position.
type At = (usize, [i32; 3]);

/// A station as the railway has it: its block, its track and shape, whether a train stands at
/// it, its assembly direction.
type Station = (
    [i32; 3],
    Option<([i32; 3], TrackShape)>,
    bool,
    Option<[i32; 3]>,
);

thread_local! {
    /// The server's: what each station's data last said, and each station's last assembly
    /// direction.
    static SENT: RefCell<HashMap<At, Shown>> = RefCell::new(HashMap::new());
    static DIRS: RefCell<HashMap<At, [i32; 2]>> = RefCell::new(HashMap::new());
    /// The server's: each signal's and observer's marker as last written.
    static MARKS: RefCell<HashMap<At, Option<Marker>>> = RefCell::new(HashMap::new());
    /// The client's flags (`StationBlockEntity.flag`): value and target as of `at` (animation
    /// ticks).
    static FLAGS: RefCell<HashMap<[i32; 3], Flag>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy, Debug)]
struct Flag {
    value: f32,
    target: f32,
    at: f64,
    /// When it last started chasing (for the wiggle), and whether it has settled.
    settled: bool,
}

pub fn forget() {
    SENT.with_borrow_mut(HashMap::clear);
    DIRS.with_borrow_mut(HashMap::clear);
    MARKS.with_borrow_mut(HashMap::clear);
}

/// A marker as the data carries it: track offset, way, kind, light.
type Marker = ([i32; 3], [i32; 2], u8, i32);

/// A marker as the railway finds it: track block, way, kind.
type Pointing = ([i32; 3], [i32; 2], u8);

/// Each signal's and track observer's marker (`TrackTargetingBehaviour.render` from
/// `SignalRenderer`, `TrackObserverRenderer`) written into its data where it changed; a signal
/// that skips (`OverlayState.SKIP`) carries none.
pub fn refresh_marks(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let marks: Vec<([i32; 3], Option<Pointing>)> = with_world(|w| {
        let r = &w.dims[dim].railway;
        let signals = r.signals.keys().map(|p| {
            let m = r
                .net
                .signal_marker(*p)
                .map(|(t, d, dual)| (t, d, u8::from(dual)));
            (*p, m)
        });
        let observers = r.observers.keys().map(|p| {
            let m = r.net.observer_marker(&r.tracks, *p).map(|(t, d)| (t, d, 2));
            (*p, m)
        });
        signals.chain(observers).collect()
    })
    .unwrap_or_default();
    for (pos, m) in marks {
        if !lv.is_loaded(pos) {
            continue;
        }
        let shown: Option<Marker> = m.map(|(t, d, k)| {
            let light = light_near(lv, t, true) as i32;
            ([t[0] - pos[0], t[1] - pos[1], t[2] - pos[2]], d, k, light)
        });
        if MARKS.with_borrow(|s| s.get(&(dim, pos)) == Some(&shown)) {
            continue;
        }
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let value = match shown {
            Some((t, d, k, l)) => Tag::compound([
                ("t", Tag::IntArray(t.to_vec())),
                ("d", Tag::IntArray(d.to_vec())),
                ("k", Tag::Byte(k as i8)),
                ("l", Tag::Int(l)),
            ]),
            None => crate::glue::kinetics::absent(),
        };
        let data = Compound::from([(MARK.into(), value)]);
        if entity.merge_data(&mut *lv.host, data).is_ok() {
            MARKS.with_borrow_mut(|s| s.insert((dim, pos), shown));
        }
    }
}

/// A signal's or observer's marker on its track, from its data (`MARK`).
pub fn draw_mark(host: &dyn ClientHost, draw: &mut dyn BlockEntityDrawContext, nbt: &Compound) {
    let Some(Tag::Compound(c)) = crate::glue::kinetics::synced(nbt, MARK) else {
        return;
    };
    let ints = |k: &str| match c.get(&k.into()) {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    };
    let (t, d) = (ints("t"), ints("d"));
    let (&[tx, ty, tz], &[dx, dz]) = (&t[..], &d[..]) else {
        return;
    };
    let name = match c.get(&"k".into()) {
        Some(Tag::Byte(1)) => SIGNAL_DUAL,
        Some(Tag::Byte(2)) => OBSERVER,
        _ => SIGNAL,
    };
    let Some(m) = model(host, name) else {
        return;
    };
    let lit = draw.light();
    if let Some(Tag::Int(l)) = c.get(&"l".into()) {
        draw.set_light(*l as u32);
    }
    draw.push();
    draw.translate([tx as f32, ty as f32, tz as f32]);
    // `prepareTrackOverlay` on straight track: turned to the way it looks (`getModelAngles`).
    let yaw = (dx as f32).atan2(dz as f32).to_degrees();
    draw.rotate_about([0.0, 1.0, 0.0], yaw, [0.5, 0.0, 0.5]);
    let _ = draw.terrain_model(m, mod_api::TerrainModelOptions::default());
    draw.pop();
    draw.set_light(lit);
}

pub fn client_reset() {
    FLAGS.with_borrow_mut(HashMap::clear);
}

/// Each station's data written again where it changed.
pub fn refresh(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let stations: Vec<Station> = with_world(|w| {
        let r = &w.dims[dim].railway;
        r.stations
            .keys()
            .map(|p| {
                let target =
                    create_engine::network::target_track(&r.tracks, *p).map(|t| (t, r.tracks[&t]));
                let present = r.trains.iter().any(|t| t.train.at_station == Some(*p));
                (*p, target, present, r.assembling.get(p).copied())
            })
            .collect()
    })
    .unwrap_or_default();
    for (pos, target, present, assembling) in stations {
        let Some((track, shape)) = target else {
            continue;
        };
        if !lv.is_loaded(pos) {
            continue;
        }
        let dir = match assembling {
            Some(d) => {
                DIRS.with_borrow_mut(|m| m.insert((dim, pos), [d[0], d[2]]));
                [d[0], d[2]]
            }
            None => DIRS.with_borrow(|m| m.get(&(dim, pos)).copied()).unwrap_or(
                if shape == TrackShape::Xo {
                    [1, 0]
                } else {
                    [0, 1]
                },
            ),
        };
        let (length, bogeys) = if assembling.is_some() {
            super::assembly::assembly_track(lv, pos)
                .map(|(run, b)| (run.len() as i32, b.iter().map(|b| b.0).collect()))
                .unwrap_or_default()
        } else {
            (0, Vec::new())
        };
        let light = [
            light_near(lv, pos, false) as i32,
            light_near(lv, track, true) as i32,
        ];
        let shown = Shown {
            light,
            track: [track[0] - pos[0], track[1] - pos[1], track[2] - pos[2]],
            dir,
            present,
            assembling: assembling.is_some(),
            length,
            bogeys,
        };
        let same = SENT.with_borrow(|m| m.get(&(dim, pos)) == Some(&shown));
        if same {
            continue;
        }
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let data = Compound::from([(KEY.into(), Tag::Compound(shown.to_nbt()))]);
        if entity.merge_data(&mut *lv.host, data).is_ok() {
            SENT.with_borrow_mut(|m| m.insert((dim, pos), shown));
        }
    }
}

/// The light at `p` (with `own`) or the brightest beside it, packed as the draw context takes
/// it: 16 times sky light in bits 0..8, 16 times block light in bits 8..16.
fn light_near(lv: &ServerLevel<'_>, p: [i32; 3], own: bool) -> u32 {
    use mod_api::LightLayer;
    let (mut sky, mut block) = (0, 0);
    let offsets: &[[i32; 3]] = if own {
        &[[0, 0, 0]]
    } else {
        &[[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, 0, 1], [0, 0, -1]]
    };
    for o in offsets {
        let q = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
        let at = |layer| lv.level.brightness(&*lv.host, layer, q).unwrap_or(0);
        sky = sky.max(at(LightLayer::Sky));
        block = block.max(at(LightLayer::Block));
    }
    let c = |v: i32| (v.clamp(0, 15) as u32) * 16;
    c(sky) | c(block) << 8
}

/// `Direction.toYRot` of a horizontal unit.
fn y_rot(d: [i32; 2]) -> f32 {
    match d {
        [0, 1] => 0.0,
        [-1, 0] => 90.0,
        [0, -1] => 180.0,
        _ => 270.0,
    }
}

/// The flag's value now, chasing its target by 0.1 a tick (`LerpedFloat.linear().chase(t, 0.1,
/// LINEAR)`), and whether it is still moving.
fn flag_now(pos: [i32; 3], target: f32, now: f64) -> (f32, bool, bool) {
    FLAGS.with_borrow_mut(|m| {
        let f = m.entry(pos).or_insert(Flag {
            value: target,
            target,
            at: now,
            settled: true,
        });
        let dt = (now - f.at).max(0.0) as f32;
        let mut value = if f.value < f.target {
            (f.value + 0.1 * dt).min(f.target)
        } else {
            (f.value - 0.1 * dt).max(f.target)
        };
        // `tick`: a new target only once the last is reached (or from 0).
        if (f.target == 0.0 || value == f.target) && target != f.target {
            f.target = target;
            f.settled = false;
        }
        f.value = value;
        f.at = now;
        if value == f.target && !f.settled && dt > 0.0 {
            // One more frame to draw the end of the wiggle.
            f.settled = true;
            return (value, true, f.target > 0.0);
        }
        value = f.value;
        (value, !f.settled, f.target > 0.0)
    })
}

/// Whether the station at `pos` (with data `nbt`) is moving its flag: drawn again every
/// frame, not kept. A station first seen shows its flag where it is.
pub fn animating(pos: [i32; 3], nbt: &Compound) -> bool {
    let Some(Tag::Compound(c)) = nbt.get(&KEY.into()) else {
        return false;
    };
    let Some(s) = Shown::of(c) else {
        return false;
    };
    let target = if s.present || s.assembling { 1.0 } else { 0.0 };
    FLAGS.with_borrow(|m| {
        m.get(&pos)
            .is_some_and(|f| !f.settled || f.value != target || f.target != target)
    })
}

fn model(host: &dyn ClientHost, id: &str) -> Option<mod_api::ModelRef> {
    let id = mod_api::Identifier::parse(id).ok()?;
    host.render_assets().model(&id).ok()
}

/// `StationRenderer.renderSafe` for the station at `pos` with data `nbt`.
pub fn draw(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    pos: [i32; 3],
    nbt: &Compound,
) {
    let Some(Tag::Compound(c)) = nbt.get(&KEY.into()) else {
        return;
    };
    let Some(s) = Shown::of(c) else {
        return;
    };
    let assembly = s.assembling && !s.present;
    let target = if s.present || s.assembling { 1.0 } else { 0.0 };
    let (value, _, chasing_up) = flag_now(pos, target, draw.animation_time());
    let flag = if assembly {
        FLAG_ASSEMBLE
    } else if value > 0.75 {
        FLAG_ON
    } else {
        FLAG_OFF
    };
    // `resolveFlagAngle`: turned to the track's way, flipped by the side the track is on.
    let flag_y_rot = -y_rot(s.dir) - 90.0;
    let clockwise = [-s.dir[1], s.dir[0]];
    let flipped = s.track[0] * clockwise[0] + s.track[2] * clockwise[1] > 0;
    // `transformFlag`: up to 90° over the first fifth, then a wiggle while it settles upward.
    let mut progress = (value * 5.0).min(1.0).powi(2);
    if chasing_up
        && !FLAGS.with_borrow(|m| m.get(&pos).is_some_and(|f| f.settled))
        && progress == 1.0
    {
        let wiggle = (value - 0.2) / 0.8;
        progress += ((wiggle * std::f32::consts::TAU * 4.0).sin() / 8.0) / (8.0 * wiggle).max(1.0);
    }
    draw.set_light(s.light[0] as u32);
    if let Some(m) = model(host, flag) {
        let nudge = 1.0 / 512.0;
        draw.push();
        draw.rotate_about([0.0, 1.0, 0.0], flag_y_rot, [0.5, 0.5, 0.5]);
        draw.translate([
            nudge,
            9.5 / 16.0,
            if flipped {
                14.0 / 16.0 - nudge
            } else {
                2.0 / 16.0 + nudge
            },
        ]);
        let sign = if flipped { 1.0 } else { -1.0 };
        draw.rotate_about([1.0, 0.0, 0.0], sign * (progress * 90.0 + 270.0), [0.0; 3]);
        draw.translate([0.5 / 16.0, 0.0, 0.0]);
        draw.rotate_about([0.0, 1.0, 0.0], if flipped { 0.0 } else { 180.0 }, [0.0; 3]);
        draw.translate([-0.5 / 16.0, 0.0, 0.0]);
        let _ = draw.terrain_model(m, mod_api::TerrainModelOptions::default());
        draw.pop();
    }
    let along = [s.dir[0] as f32, s.dir[1] as f32];
    draw.set_light(s.light[1] as u32);
    draw.push();
    draw.translate([s.track[0] as f32, s.track[1] as f32, s.track[2] as f32]);
    if !assembly {
        // The station's marker on its track, turned along it.
        if let Some(m) = model(host, OVERLAY) {
            let yaw = along[0].atan2(along[1]).to_degrees();
            draw.rotate_about([0.0, 1.0, 0.0], yaw, [0.5, 0.0, 0.5]);
            let _ = draw.terrain_model(m, mod_api::TerrainModelOptions::default());
        }
        draw.pop();
        keep(draw, pos, nbt);
        return;
    }
    // `prepareAssemblyOverlay`: turned to the assembly direction, one block on per step.
    let angle = y_rot(s.dir);
    draw.rotate_about([0.0, 1.0, 0.0], angle, [0.5, 0.5, 0.5]);
    let count = s.bogeys.len();
    for i in 0..s.length {
        draw.translate([0.0, 0.0, 1.0]);
        let colour = if s.bogeys.contains(&i) {
            Some(0xCA_FF96)
        } else if valid_bogey_offset(i, count, &s.bogeys) {
            Some(0x96_B5FF)
        } else {
            None
        };
        // `SuperByteBuffer.color`, through the tinted copy's tint index.
        if let Some(colour) = colour
            && let Some(m) = model(host, ASSEMBLING)
        {
            let mut options = mod_api::TerrainModelOptions::default();
            options.tint = colour;
            let _ = draw.terrain_model(m, options);
        }
    }
    draw.pop();
    keep(draw, pos, nbt);
}

/// The drawing is kept until the data changes, unless the flag moves (drawn again next frame).
fn keep(draw: &mut dyn BlockEntityDrawContext, pos: [i32; 3], nbt: &Compound) {
    if !animating(pos, nbt) {
        draw.keep();
    }
}

/// `isValidBogeyOffset`: at least 3 on from the station's track, and 3 clear of each bogey.
fn valid_bogey_offset(i: i32, count: usize, bogeys: &[i32]) -> bool {
    if (i < 3 || count == 0) && i != 0 {
        return false;
    }
    !bogeys.iter().any(|j| i >= j - 2 && i <= j + 2)
}
