//! What the redstone blocks keep in their block entities: redstone links, brass diodes, analog
//! levers, smart observers and threshold switches. An older version kept them with the column
//! (`links`, `diodes`, `create_redstone`), read once by [`load`].

use crate::glue::entity_data::Entities;
use crate::glue::items::filters;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::filter::Filter;
use create_engine::observe::{Observer, Switch};
use mod_api::{Compound, Tag};

/// What an older version saved with the column, before the column's blocks are found.
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

const FREQUENCY_FIRST: &str = "FrequencyFirst";
const FREQUENCY_LAST: &str = "FrequencyLast";
const TRANSMIT: &str = "Transmit";
const STATE: &str = "State";
const SCROLL_VALUE: &str = "ScrollValue";
const CHANGE_TIMER: &str = "ChangeTimer";
const FILTER: &str = "Filter";
const TURN_OFF: &str = "TurnOff";
const SWITCH_KEYS: [&str; 10] = [
    FILTER,
    "OnAboveAmount",
    "OffBelowAmount",
    "Inverted",
    "CurrentAmount",
    "CurrentMinAmount",
    "CurrentMaxAmount",
    "InStacks",
    "Powered",
    "PoweredAfterDelay",
];

/// The keys a redstone block entity of `block` saves.
pub(super) fn saved_keys(block: Block) -> &'static [&'static str] {
    match block {
        Block::RedstoneLink => &[FREQUENCY_FIRST, FREQUENCY_LAST, TRANSMIT],
        Block::AnalogLever => &[STATE, CHANGE_TIMER],
        Block::ContentObserver => &[FILTER, TURN_OFF],
        Block::StockpileSwitch => &SWITCH_KEYS,
        b if create_engine::world::diode_kind(b).is_some() => &[STATE, SCROLL_VALUE],
        _ => &[],
    }
}

/// `RedstoneLinkBlockEntity.write`, `BrassDiodeBlockEntity.write`,
/// `AnalogLeverBlockEntity.write`, `SmartObserverBlockEntity.write` and
/// `ThresholdSwitchBlockEntity.write` of column `chunk`'s block entities.
pub(super) fn save(lv: &ServerLevel<'_>, chunk: [i32; 2], out: &mut Entities) {
    let dim = lv.dim;
    let now = lv.game_time();
    let Some((links, diodes, levers, (observers, switches))) = with_world(|w| {
        let d = &w.dims[dim];
        (
            w.column_links(dim, chunk[0], chunk[1]),
            w.column_diodes(dim, chunk[0], chunk[1]),
            d.levers.column(chunk[0], chunk[1]),
            d.detectors.column(chunk[0], chunk[1]),
        )
    }) else {
        return;
    };
    for (pos, key, transmit) in links {
        out.entry(pos).or_default().extend([
            (FREQUENCY_FIRST.into(), frequency_to_nbt(&key.0)),
            (FREQUENCY_LAST.into(), frequency_to_nbt(&key.1)),
            (TRANSMIT.into(), Tag::Int(transmit)),
        ]);
    }
    for (pos, state, max) in diodes {
        out.entry(pos).or_default().extend([
            (STATE.into(), Tag::Int(state)),
            (SCROLL_VALUE.into(), Tag::Int(max)),
        ]);
    }
    for (pos, l) in levers {
        let left = l.due.map_or(0, |t| (t - now).clamp(0, 15) as i32);
        out.entry(pos).or_default().extend([
            (STATE.into(), Tag::Int(i32::from(l.state))),
            (CHANGE_TIMER.into(), Tag::Int(left)),
        ]);
    }
    for (pos, o) in observers {
        let left = o.off_at.map_or(0, |t| (t - now).clamp(0, 64) as i32);
        out.entry(pos).or_default().extend([
            (FILTER.into(), filter_to_nbt(lv, o.filter.as_ref())),
            (TURN_OFF.into(), Tag::Int(left)),
        ]);
    }
    for (pos, x) in switches {
        let flag = |b: bool| Tag::Byte(i8::from(b));
        out.entry(pos)
            .or_default()
            .extend(SWITCH_KEYS.into_iter().map(Into::into).zip([
                filter_to_nbt(lv, x.filter.as_ref()),
                Tag::Int(x.on_above),
                Tag::Int(x.off_below),
                flag(x.inverted),
                Tag::Int(x.level),
                Tag::Int(x.min),
                Tag::Int(x.max),
                flag(x.in_stacks),
                flag(x.state),
                flag(x.powered),
            ]));
    }
}

/// The `read`s of the block entities [`save`] writes.
pub(super) fn load_entity(lv: &mut ServerLevel<'_>, pos: [i32; 3], block: Block, tag: &Compound) {
    let dim = lv.dim;
    let now = lv.game_time();
    let int = |k: &str| match tag.get(&k.into()) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    };
    let flag = |k: &str| matches!(tag.get(&k.into()), Some(Tag::Byte(b)) if *b != 0);
    let filter =
        |lv: &ServerLevel<'_>| tag.get(&FILTER.into()).and_then(|t| filter_from_nbt(lv, t));
    match block {
        Block::RedstoneLink => {
            let key = (
                frequency_from_nbt(tag.get(&FREQUENCY_FIRST.into())),
                frequency_from_nbt(tag.get(&FREQUENCY_LAST.into())),
            );
            let transmit = int(TRANSMIT).clamp(0, 15);
            with_world(|w| w.read_link(dim, pos, key, transmit));
        }
        Block::AnalogLever => {
            let left = int(CHANGE_TIMER);
            let lever = create_engine::lever::Lever {
                state: int(STATE).clamp(0, 15) as u8,
                due: (left > 0).then(|| now + i64::from(left.min(15))),
            };
            with_world(|w| w.dims[dim].levers.read(pos, lever));
        }
        Block::ContentObserver => {
            let left = int(TURN_OFF);
            let o = Observer {
                filter: filter(lv),
                sustained: None,
                off_at: (left > 0).then(|| now + i64::from(left.min(64))),
            };
            with_world(|w| w.dims[dim].detectors.read_observer(pos, o));
        }
        Block::StockpileSwitch => {
            let s = Switch {
                on_above: int(SWITCH_KEYS[1]),
                off_below: int(SWITCH_KEYS[2]),
                inverted: flag(SWITCH_KEYS[3]),
                level: int(SWITCH_KEYS[4]),
                min: int(SWITCH_KEYS[5]),
                max: int(SWITCH_KEYS[6]),
                in_stacks: flag(SWITCH_KEYS[7]),
                state: flag(SWITCH_KEYS[8]),
                powered: flag(SWITCH_KEYS[9]),
                filter: filter(lv),
            };
            with_world(|w| w.dims[dim].detectors.read_switch(pos, s));
        }
        b if create_engine::world::diode_kind(b).is_some() => {
            let (state, max) = (int(STATE), int(SCROLL_VALUE));
            with_world(|w| w.read_diode(dim, pos, state, max));
        }
        _ => {}
    }
}

/// `LinkBehaviour.write`'s frequency slot: the item's stack, empty for none.
fn frequency_to_nbt(item: &str) -> Tag {
    if item.is_empty() {
        return Tag::Compound(Compound::new());
    }
    Tag::Compound(Compound::from([
        ("id".into(), Tag::String(item.into())),
        ("count".into(), Tag::Int(1)),
    ]))
}

fn frequency_from_nbt(tag: Option<&Tag>) -> String {
    match tag {
        Some(Tag::Compound(c)) => match c.get(&"id".into()) {
            Some(Tag::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// The `links` an older version saved with a column.
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

/// The `diodes` an older version saved with a column: position, then `State` and the delay.
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

type Saved = (
    Vec<([i32; 3], create_engine::lever::Lever)>,
    Vec<([i32; 3], Observer)>,
    Vec<([i32; 3], Switch)>,
);

/// The `create_redstone` an older version saved with the column, for the blocks found as the
/// column loads (at game time `now`).
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
