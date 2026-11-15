//! What the redstone blocks keep with a column: redstone links
//! (`links`) and brass diodes (`diodes`) beside the kinetic settings, and analog levers, smart
//! observers and threshold switches under `create_redstone`.

use crate::glue::items::filters;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::filter::Filter;
use create_engine::observe::{Observer, Switch};
use mod_api::{Compound, Tag};

/// Reads what `save` wrote, before the column's blocks are found.
pub(super) fn load(lv: &mut ServerLevel<'_>, data: &Compound) {
    let dim = lv.dim;
    let now = lv.game_time();
    let links = links_from_nbt(data);
    let diodes = diodes_from_nbt(data);
    let redstone = match data.get(&"create_redstone".into()) {
        Some(Tag::Compound(c)) => Some(redstone_from_nbt(lv, now, c)),
        _ => None,
    };
    with_world(|w| {
        if !links.is_empty() {
            w.load_links(dim, links.into_iter());
        }
        if !diodes.is_empty() {
            w.load_diodes(dim, diodes.into_iter());
        }
        if let Some((levers, observers, switches)) = redstone {
            let d = &mut w.dims[dim];
            d.levers.load(levers.into_iter());
            d.detectors
                .load(observers.into_iter(), switches.into_iter());
        }
    });
}

/// The column's links, diodes, levers, observers and switches.
pub(super) fn save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    let dim = lv.dim;
    let Some((links, diodes)) = with_world(|w| {
        (
            w.column_links(dim, chunk[0], chunk[1]),
            w.column_diodes(dim, chunk[0], chunk[1]),
        )
    }) else {
        return;
    };
    if !links.is_empty() {
        data.insert("links".into(), links_to_nbt(&links));
    }
    if !diodes.is_empty() {
        // Brass diodes (`BrassDiodeBlockEntity.write`): x, y, z, `State` and the delay each.
        let v: Vec<i32> = diodes
            .iter()
            .flat_map(|(p, state, max)| [p[0], p[1], p[2], *state, *max])
            .collect();
        data.insert("diodes".into(), Tag::IntArray(v));
    }
    redstone_chunk_save(lv, chunk, data);
}

/// `LinkBehaviour.write` (`FrequencyFirst`, `FrequencyLast`) and `Transmit`, per link.
fn links_to_nbt(links: &[([i32; 3], create_engine::link::Key, i32)]) -> Tag {
    Tag::List(
        links
            .iter()
            .map(|(p, key, transmit)| {
                let mut b = Compound::new();
                b.insert(
                    "pos".into(),
                    Tag::IntArray(vec![p[0], p[1], p[2], *transmit]),
                );
                b.insert("FrequencyFirst".into(), Tag::String(key.0.as_str().into()));
                b.insert("FrequencyLast".into(), Tag::String(key.1.as_str().into()));
                Tag::Compound(b)
            })
            .collect(),
    )
}

fn links_from_nbt(data: &Compound) -> Vec<([i32; 3], (create_engine::link::Key, i32))> {
    let Some(Tag::List(list)) = data.get(&"links".into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let Tag::Compound(b) = b else { return None };
            let Some(Tag::IntArray(pos)) = b.get(&"pos".into()) else {
                return None;
            };
            let item = |k: &str| match b.get(&k.into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => String::new(),
            };
            Some((
                [*pos.first()?, *pos.get(1)?, *pos.get(2)?],
                (
                    (item("FrequencyFirst"), item("FrequencyLast")),
                    (*pos.get(3)?).clamp(0, 15),
                ),
            ))
        })
        .collect()
}

/// Brass diodes saved with a column: position, then `State` and the delay.
fn diodes_from_nbt(data: &Compound) -> Vec<([i32; 3], (i32, i32))> {
    let Some(Tag::IntArray(v)) = data.get(&"diodes".into()) else {
        return Vec::new();
    };
    v.as_chunks::<5>()
        .0
        .iter()
        .map(|c| ([c[0], c[1], c[2]], (c[3], c[4])))
        .collect()
}

/// A detector's filter as saved: its stack (`filters::filter_to_nbt`), or "" for none.
fn filter_to_nbt(lv: &ServerLevel<'_>, f: Option<&Filter>) -> Tag {
    f.and_then(|f| filters::filter_to_nbt(&*lv.host, f))
        .unwrap_or_else(|| Tag::String("".into()))
}

/// What `filter_to_nbt` wrote, read as a filter item is (`filters::filter_from_nbt`).
fn filter_from_nbt(lv: &ServerLevel<'_>, tag: &Tag) -> Option<Filter> {
    filters::filter_from_nbt(&*lv.host, tag)
}

/// Analog levers, smart observers and threshold switches saved with a column
/// (`create_redstone`): levers' `State` and `ChangeTimer`; observers' filters and `TurnOff`;
/// switches' thresholds, `Inverted`, `InStacks`, levels, `Powered`, `PoweredAfterDelay` and
/// filters.
fn redstone_chunk_save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    let dim = lv.dim;
    let now = lv.game_time();
    let Some((levers, (observers, switches))) = with_world(|w| {
        let d = &w.dims[dim];
        (
            d.levers.column(chunk[0], chunk[1]),
            d.detectors.column(chunk[0], chunk[1]),
        )
    }) else {
        return;
    };
    if levers.is_empty() && observers.is_empty() && switches.is_empty() {
        return;
    }
    let mut own = Compound::new();
    if !levers.is_empty() {
        let v = levers
            .iter()
            .flat_map(|(p, l)| {
                let left = l.due.map_or(0, |t| (t - now).clamp(0, 15) as i32);
                [p[0], p[1], p[2], i32::from(l.state), left]
            })
            .collect();
        own.insert("levers".into(), Tag::IntArray(v));
    }
    if !observers.is_empty() {
        let mut c = Compound::new();
        let nums = observers
            .iter()
            .flat_map(|(p, o)| {
                let left = o.off_at.map_or(-1, |t| (t - now).clamp(0, 64) as i32);
                [p[0], p[1], p[2], left]
            })
            .collect();
        c.insert("nums".into(), Tag::IntArray(nums));
        let filters = observers
            .iter()
            .map(|(_, o)| filter_to_nbt(lv, o.filter.as_ref()))
            .collect();
        c.insert("filters".into(), Tag::List(filters));
        own.insert("observers".into(), Tag::Compound(c));
    }
    if !switches.is_empty() {
        let mut c = Compound::new();
        let nums = switches
            .iter()
            .flat_map(|(p, x)| {
                let flags = i32::from(x.inverted)
                    | i32::from(x.in_stacks) << 1
                    | i32::from(x.state) << 2
                    | i32::from(x.powered) << 3;
                [
                    p[0],
                    p[1],
                    p[2],
                    x.on_above,
                    x.off_below,
                    flags,
                    x.level,
                    x.min,
                    x.max,
                ]
            })
            .collect();
        c.insert("nums".into(), Tag::IntArray(nums));
        let filters = switches
            .iter()
            .map(|(_, x)| filter_to_nbt(lv, x.filter.as_ref()))
            .collect();
        c.insert("filters".into(), Tag::List(filters));
        own.insert("switches".into(), Tag::Compound(c));
    }
    data.insert("create_redstone".into(), Tag::Compound(own));
}

type Saved = (
    Vec<([i32; 3], create_engine::lever::Lever)>,
    Vec<([i32; 3], Observer)>,
    Vec<([i32; 3], Switch)>,
);

/// What `redstone_chunk_save` wrote, for the blocks found as the column loads (at game time
/// `now`).
fn redstone_from_nbt(lv: &ServerLevel<'_>, now: i64, data: &Compound) -> Saved {
    let part = |key: &str| -> Option<(Vec<i32>, Vec<Option<Filter>>)> {
        let Some(Tag::Compound(c)) = data.get(&key.into()) else {
            return None;
        };
        let nums = match c.get(&"nums".into()) {
            Some(Tag::IntArray(v)) => v.clone(),
            _ => return None,
        };
        let filters = match c.get(&"filters".into()) {
            Some(Tag::List(l)) => l.iter().map(|t| filter_from_nbt(lv, t)).collect(),
            _ => Vec::new(),
        };
        Some((nums, filters))
    };
    let levers = match data.get(&"levers".into()) {
        Some(Tag::IntArray(v)) => v
            .as_chunks::<5>()
            .0
            .iter()
            .map(|c| {
                let lever = create_engine::lever::Lever {
                    state: c[3].clamp(0, 15) as u8,
                    due: (c[4] > 0).then(|| now + i64::from(c[4].min(15))),
                };
                ([c[0], c[1], c[2]], lever)
            })
            .collect(),
        _ => Vec::new(),
    };
    let observers = part("observers")
        .map(|(nums, filters)| {
            nums.as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let o = Observer {
                        filter: filters.get(i).cloned().flatten(),
                        sustained: None,
                        off_at: (c[3] >= 0).then(|| now + i64::from(c[3])),
                    };
                    ([c[0], c[1], c[2]], o)
                })
                .collect()
        })
        .unwrap_or_default();
    let switches = part("switches")
        .map(|(nums, filters)| {
            nums.as_chunks::<9>()
                .0
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let s = Switch {
                        on_above: c[3],
                        off_below: c[4],
                        inverted: c[5] & 1 != 0,
                        in_stacks: c[5] & 2 != 0,
                        state: c[5] & 4 != 0,
                        powered: c[5] & 8 != 0,
                        level: c[6],
                        min: c[7],
                        max: c[8],
                        filter: filters.get(i).cloned().flatten(),
                    };
                    ([c[0], c[1], c[2]], s)
                })
                .collect()
        })
        .unwrap_or_default();
    (levers, observers, switches)
}
