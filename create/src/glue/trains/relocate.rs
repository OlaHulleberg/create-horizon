//! Moving a train with the wrench (`TrainRelocator`): a wrench's click on a standing carriage
//! picks its train, the next click on a track within 24 blocks of where it was picked puts the
//! train there, facing the way the player looks; sneaking or letting go of the wrench gives
//! up. Create picks and previews on the client; here the server keeps the pick, the action bar
//! says what a click does, and while a picking player looks at track the server sends them where
//! the wheels would go (`preview`, `TrainRelocator.relocateClient(.., simulate)`), which their
//! client draws as Create's green line, its last piece red where the train won't fit.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::railway::{railway_ready, send_trains};
use mod_api::{PlayerRef, ServerHost, Text};
use std::cell::RefCell;
use std::collections::HashMap;

/// A player's pick: the level, the train, and where the carriage was clicked.
#[derive(Clone, Copy)]
struct Pick {
    dim: usize,
    train: u32,
    origin: [f64; 3],
}

/// A track block looked at, and the way (the signs of x and z from the eye to it).
type Looked = ([i32; 3], [i32; 2]);

thread_local! {
    static PICKS: RefCell<HashMap<u128, Pick>> = RefCell::new(HashMap::new());
    /// What each picking player was last sent: the track looked at (and the way), or none.
    static SENT: RefCell<HashMap<u128, Option<Looked>>> = RefCell::new(HashMap::new());
    /// The track block each player's crosshair is on, as their client says (`H`).
    static HOVERS: RefCell<HashMap<u128, [i32; 3]>> = RefCell::new(HashMap::new());
    /// The client's: the track block it last told of.
    static TOLD: std::cell::Cell<Option<[i32; 3]>> = const { std::cell::Cell::new(None) };
    /// The client's preview: the wheels' places, front first, and whether the train fits.
    static PREVIEW: RefCell<Option<(Vec<[f64; 3]>, bool)>> = const { RefCell::new(None) };
}

/// The preview's channel, server to client: none (empty), or whether it fits and the points.
pub const CHANNEL: &str = "create:train_relocation";

pub fn forget() {
    PICKS.with_borrow_mut(HashMap::clear);
    SENT.with_borrow_mut(HashMap::clear);
    HOVERS.with_borrow_mut(HashMap::clear);
}

pub fn client_reset() {
    PREVIEW.with_borrow_mut(|p| *p = None);
    TOLD.set(None);
}

/// A client tick: with a wrench in hand, the track block under the crosshair is told to the
/// server when it changes (`relocateClient` reads `mc.hitResult`; the server keeps the pick).
pub fn client_tick(host: &mut dyn mod_api::ClientHost) {
    let wrench = host
        .local_player()
        .and_then(|p| crate::glue::items::held(&*host, p, mod_api::Hand::Main))
        .is_some_and(|s| s.item().id(&*host) == "create:wrench");
    let track = match host.crosshair_target() {
        Some(mod_api::CrosshairTarget::Block { pos, state, .. }) if wrench => {
            let id = super::super::states::id(state);
            let is_track = id
                .and_then(|id| {
                    super::super::client_table(&*host, |t| t.get(id).map(|(b, _)| b)).flatten()
                })
                .is_some_and(|b| b == create_engine::Block::Track);
            is_track.then_some([pos.x, pos.y, pos.z])
        }
        _ => None,
    };
    if TOLD.get() == track {
        return;
    }
    TOLD.set(track);
    let mut m = vec![b'H'];
    if let Some(p) = track {
        for v in p {
            m.extend(v.to_le_bytes());
        }
    }
    crate::glue::net::send_to_server(host, CHANNEL, &m);
}

/// A client's word: the track block its crosshair is on (`H`), or none.
pub fn client_message(player: PlayerRef, data: &[u8]) {
    let Some((b'H', rest)) = data.split_first().map(|(k, r)| (*k, r)) else {
        return;
    };
    let ints: Vec<i32> = rest
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| i32::from_le_bytes(*c))
        .collect();
    HOVERS.with_borrow_mut(|h| match ints[..] {
        [x, y, z] => {
            h.insert(player.uuid(), [x, y, z]);
        }
        _ => {
            h.remove(&player.uuid());
        }
    });
}

/// Each picking player of `lv` with a wrench: the track they look at (as their client says) and
/// where the picked train's wheels would go on it, sent when what they look at changes. A pick
/// given up stops the preview.
pub fn preview(lv: &mut ServerLevel<'_>) {
    let picks: Vec<(u128, Pick)> = PICKS.with_borrow(|p| p.iter().map(|(k, v)| (*k, *v)).collect());
    let gone: Vec<u128> = SENT.with_borrow(|s| {
        s.keys()
            .filter(|k| !picks.iter().any(|(p, _)| p == *k))
            .copied()
            .collect()
    });
    for uuid in gone {
        SENT.with_borrow_mut(|s| s.remove(&uuid));
        if let Some(player) = lv.host.player(uuid) {
            crate::glue::net::send_to_player(lv.host, player, CHANNEL, &[]);
        }
    }
    for (uuid, pick) in picks {
        if pick.dim != lv.dim {
            continue;
        }
        let Some(player) = lv.host.player(uuid) else {
            continue;
        };
        let host = &*lv.host;
        let wrench = crate::glue::items::held(host, player, mod_api::Hand::Main)
            .is_some_and(|s| s.item().id(host) == "create:wrench");
        let entity = player.entity();
        let hovered = HOVERS.with_borrow(|h| h.get(&uuid).copied());
        let aimed = wrench
            .then(|| {
                let block = hovered?;
                let eye = entity.eye_position(host)?;
                // The way `track_clicked` takes: from the eye to the block, flat.
                let way = [
                    f64::from(block[0]) + 0.5 - eye[0],
                    f64::from(block[2]) + 0.5 - eye[2],
                ];
                Some((block, way))
            })
            .flatten();
        let key = aimed.map(|(b, w)| (b, w.map(|v| v.signum() as i32)));
        if SENT.with_borrow(|s| s.get(&uuid) == Some(&key)) {
            continue;
        }
        SENT.with_borrow_mut(|s| s.insert(uuid, key));
        let dim = lv.dim;
        railway_ready(dim);
        let shown = aimed.and_then(|(block, way)| {
            with_world(|w| {
                let r = &w.dims[dim].railway;
                let t = r.trains.iter().find(|t| t.id == pick.train)?;
                t.train.relocation(&r.net, block, [way[0], 0.0, way[1]])
            })
            .flatten()
        });
        let mut m = Vec::new();
        if let Some((points, ok)) = shown {
            m.push(u8::from(ok));
            for p in points {
                for v in p {
                    m.extend(v.to_le_bytes());
                }
            }
        }
        crate::glue::net::send_to_player(lv.host, player, CHANNEL, &m);
    }
}

/// The server's preview, for the client.
pub fn server_message(data: &[u8]) {
    let Some((&ok, rest)) = data.split_first() else {
        PREVIEW.with_borrow_mut(|p| *p = None);
        return;
    };
    let points: Vec<[f64; 3]> = rest
        .as_chunks::<24>()
        .0
        .iter()
        .map(|c| {
            let v = |i: usize| f64::from_le_bytes(c[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
            [v(0), v(1), v(2)]
        })
        .collect();
    PREVIEW.with_borrow_mut(|p| *p = Some((points, ok != 0)));
}

/// `relocateClient`'s lines (`Outliner.showLine`): boxes between each wheel's place and the
/// next, 0.925 below them, a quarter block thick for a bogey's own pair and a sixth across the
/// gap to the next, green (`0x95CD41`), the last red (`0xEA5C2B`) where the train won't fit. Each
/// is drawn as its box's top face and its centre line.
pub fn render_level_stage(event: &mut mod_api::RenderLevelStageEvent<'_>) {
    if event.stage != mod_api::RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let (quads, lines) = PREVIEW.with_borrow(|p| {
        let mut quads = Vec::new();
        let mut lines = Vec::new();
        let Some((points, ok)) = p else {
            return (quads, lines);
        };
        let n = points.len();
        for i in 0..n.saturating_sub(1) {
            let down = |v: [f64; 3]| [v[0], v[1] - f64::from(0.925f32), v[2]];
            let (a, b) = (down(points[i]), down(points[i + 1]));
            let colour = if *ok || i != n - 2 {
                0xFF95_CD41
            } else {
                0xFFEA_5C2B
            };
            let w = if i % 2 == 1 { 1.0 / 6.0 } else { 1.0 / 4.0 };
            let d = [b[0] - a[0], b[2] - a[2]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
            if l > 1e-6 {
                let side = [-d[1] / l * w / 2.0, d[0] / l * w / 2.0];
                let top = w / 2.0;
                let c = |p: [f64; 3], s: f64| [p[0] + side[0] * s, p[1] + top, p[2] + side[1] * s];
                // Both windings: seen from above and below.
                for corners in [
                    [c(a, -1.0), c(a, 1.0), c(b, 1.0), c(b, -1.0)],
                    [c(b, -1.0), c(b, 1.0), c(a, 1.0), c(a, -1.0)],
                ] {
                    quads.push(mod_api::WorldQuad::new(
                        corners,
                        colour,
                        mod_api::WorldDepth::Tested,
                    ));
                }
            }
            lines.push(mod_api::WorldLine::new(
                a,
                b,
                colour,
                2.0,
                mod_api::WorldDepth::Tested,
            ));
        }
        (quads, lines)
    });
    if !quads.is_empty() {
        let _ = event.draw.quads(&quads);
    }
    if !lines.is_empty() {
        let _ = event.draw.lines(&lines);
    }
}

fn say(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    key: &str,
    args: Vec<Text>,
    colour: Option<&str>,
) {
    let mut text = Text::translate(key, args);
    if let Some(c) = colour {
        let mut style = mod_api::text::TextStyle::default();
        style.color = Some(c.to_owned());
        text = text.with_style(style);
    }
    super::action_bar(host, player, text);
}

/// `carriageWrenched`: the wrench's click on train `train` at `origin`. A moving train can't be
/// picked (`train.cannot_relocate_moving`).
pub fn wrenched(
    host: &mut dyn ServerHost,
    dim: usize,
    player: PlayerRef,
    train: u32,
    origin: [f64; 3],
) {
    let found = with_world(|w| {
        w.dims[dim]
            .railway
            .trains
            .iter()
            .find(|t| t.id == train)
            .map(|t| (t.train.speed != 0.0, t.name.clone()))
    })
    .flatten();
    let Some((moving, name)) = found else {
        return;
    };
    if moving {
        say(
            host,
            player,
            "create.train.cannot_relocate_moving",
            Vec::new(),
            Some("red"),
        );
        return;
    }
    PICKS.with_borrow_mut(|p| p.insert(player.uuid(), Pick { dim, train, origin }));
    let name = if name.is_empty() {
        Text::translate("create.train.unnamed", Vec::new())
    } else {
        Text::literal(name)
    };
    say(host, player, "create.train.relocate", vec![name], None);
}

/// Whether `player` has a train picked.
pub fn picking(player: PlayerRef) -> bool {
    PICKS.with_borrow(|p| p.contains_key(&player.uuid()))
}

/// The wrench's click on track `pos` with a train picked (`onClicked`, `TrainRelocationPacket`):
/// sneaking gives up; too far from the pick is refused; else the train goes there if it fits.
/// True when the click was the relocator's.
pub fn track_clicked(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) -> bool {
    let Some(pick) = PICKS.with_borrow(|p| p.get(&player.uuid()).copied()) else {
        return false;
    };
    let host = &mut *lv.host;
    let wrench = crate::glue::items::held(&*host, player, mod_api::Hand::Main)
        .is_some_and(|s| s.item().id(&*host) == "create:wrench");
    if crate::glue::items::sneaking(&*host, player) || !wrench || pick.dim != lv.dim {
        PICKS.with_borrow_mut(|p| p.remove(&player.uuid()));
        say(
            host,
            player,
            "create.train.relocate.abort",
            Vec::new(),
            Some("red"),
        );
        return true;
    }
    // `canInteractWithBlock(relocatingOrigin, 24)`.
    let near = player.entity().position(&*host).is_some_and(|p| {
        let d: f64 = (0..3).map(|i| (p[i] - pick.origin[i]).powi(2)).sum();
        d < 24.0 * 24.0
    });
    if !near {
        say(
            host,
            player,
            "create.train.relocate.too_far",
            Vec::new(),
            Some("red"),
        );
        return true;
    }
    // The way the player looks at the clicked track (`lookAngle`): from their eye to it.
    let look = player
        .entity()
        .position(&*host)
        .map_or([0.0, 0.0, 1.0], |p| {
            [
                f64::from(pos[0]) + 0.5 - p[0],
                0.0,
                f64::from(pos[2]) + 0.5 - p[2],
            ]
        });
    let dim = lv.dim;
    railway_ready(dim);
    let moved = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let net = &r.net;
        let t = r.trains.iter_mut().find(|t| t.id == pick.train)?;
        let was = t.column;
        if t.train.speed != 0.0 || !t.train.relocate(net, pos, look) {
            return Some(None);
        }
        t.runtime.interrupted();
        t.column = t.column_on(net);
        t.fresh = true;
        Some(Some((was, t.column)))
    })
    .flatten();
    match moved {
        Some(Some((was, now))) => {
            PICKS.with_borrow_mut(|p| p.remove(&player.uuid()));
            super::super::persist::mark_unsaved(dim, [was.0, was.1]);
            super::super::persist::mark_unsaved(dim, [now.0, now.1]);
            say(
                lv.host,
                player,
                "create.train.relocate.success",
                Vec::new(),
                Some("green"),
            );
            send_trains(lv, &[pick.train], true);
        }
        Some(None) => {
            say(
                lv.host,
                player,
                "create.train.relocate.invalid",
                Vec::new(),
                Some("red"),
            );
        }
        None => {
            PICKS.with_borrow_mut(|p| p.remove(&player.uuid()));
        }
    }
    true
}
