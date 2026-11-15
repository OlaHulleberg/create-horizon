//! What the redstone blocks keep for a block that rides a moving structure
//! (`Subsystem::carry_out`, Create's block entity NBT): a redstone link's frequencies and what
//! it transmits (`LinkBehaviour.write`), a brass diode's `State` and delay, an analog lever's
//! `State`, a smart observer's filter and a threshold switch's settings and filter. Each leaves
//! its network or table as it goes and waits where it goes back as a saved one does.

use crate::glue::items::filters;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::observe::{Observer, Switch};
use mod_api::{Compound, Tag};

const LINK: &str = "LinkFrequency";
const LINK_TRANSMIT: &str = "LinkTransmit";
const DIODE: &str = "Diode";
const LEVER: &str = "Lever";
const OBSERVER: &str = "ObserverFilter";
const SWITCH: &str = "Switch";
const SWITCH_FILTER: &str = "SwitchFilter";

pub(super) fn carry_out(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block, out: &mut Compound) {
    let dim = lv.dim;
    let now = lv.game_time();
    let host = &*lv.host;
    let filter_tag = |f: &create_engine::filter::Filter| filters::filter_to_nbt(host, f);
    match block {
        Block::RedstoneLink => {
            let link = with_world(|w| {
                let d = &mut w.dims[dim];
                let l = d.links.links.get(&pos).cloned()?;
                // `LinkBehaviour.unload`: it leaves its network.
                let changed = d.links.remove(pos);
                d.link_changes.extend(changed);
                Some(l)
            })
            .flatten();
            if let Some(l) = link {
                out.insert(
                    LINK.into(),
                    Tag::List(vec![
                        Tag::String(l.key.0.as_str().into()),
                        Tag::String(l.key.1.as_str().into()),
                    ]),
                );
                out.insert(LINK_TRANSMIT.into(), Tag::Int(l.transmit));
            }
        }
        Block::PulseRepeater | Block::PulseExtender | Block::PulseTimer => {
            let diode = with_world(|w| {
                let d = &mut w.dims[dim];
                let x = d.diodes.remove(&pos)?;
                d.diode_running.retain(|p| *p != pos);
                Some((x.state, x.max))
            })
            .flatten();
            if let Some((state, max)) = diode {
                out.insert(DIODE.into(), Tag::IntArray(vec![state, max]));
            }
        }
        Block::AnalogLever => {
            let lever = with_world(|w| {
                let l = &mut w.dims[dim].levers;
                let x = l.levers.remove(&pos)?;
                if let Some(t) = x.due {
                    l.due.remove(&(t, pos));
                }
                Some(x)
            })
            .flatten();
            if let Some(l) = lever {
                let left = l.due.map_or(0, |t| (t - now).clamp(0, 15) as i32);
                out.insert(LEVER.into(), Tag::IntArray(vec![i32::from(l.state), left]));
            }
        }
        Block::ContentObserver | Block::StockpileSwitch => {
            let taken = with_world(|w| {
                let d = &mut w.dims[dim].detectors;
                let o = d.observers.get(&pos).cloned();
                let s = d.switches.get(&pos).cloned();
                // Gone from its target's watchers; its filter does not drop.
                d.removed(pos);
                d.dropped.retain(|(p, _)| *p != pos);
                (o, s)
            });
            let Some((o, s)) = taken else {
                return;
            };
            if let Some(f) = o.and_then(|o| o.filter).as_ref().and_then(filter_tag) {
                out.insert(OBSERVER.into(), f);
            }
            if let Some(s) = s {
                let flags = i32::from(s.inverted)
                    | i32::from(s.in_stacks) << 1
                    | i32::from(s.state) << 2
                    | i32::from(s.powered) << 3;
                out.insert(
                    SWITCH.into(),
                    Tag::IntArray(vec![s.on_above, s.off_below, flags, s.level, s.min, s.max]),
                );
                if let Some(f) = s.filter.as_ref().and_then(filter_tag) {
                    out.insert(SWITCH_FILTER.into(), f);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn carry_in(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block, data: &Compound) {
    let dim = lv.dim;
    let now = lv.game_time();
    let host = &*lv.host;
    let ints = |k: &str| match data.get(&k.into()) {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    };
    let filter = |k: &str| {
        data.get(&k.into())
            .and_then(|t| filters::filter_from_nbt(host, t))
    };
    match block {
        Block::RedstoneLink => {
            let Some(Tag::List(key)) = data.get(&LINK.into()) else {
                return;
            };
            let text = |i: usize| match key.get(i) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => String::new(),
            };
            let transmit = match data.get(&LINK_TRANSMIT.into()) {
                Some(Tag::Int(v)) => (*v).clamp(0, 15),
                _ => 0,
            };
            let saved = (pos, ((text(0), text(1)), transmit));
            with_world(|w| w.load_links(dim, std::iter::once(saved)));
        }
        Block::PulseRepeater | Block::PulseExtender | Block::PulseTimer => {
            if let [state, max] = ints(DIODE)[..] {
                with_world(|w| w.load_diodes(dim, std::iter::once((pos, (state, max)))));
            }
        }
        Block::AnalogLever => {
            if let [state, left] = ints(LEVER)[..] {
                let l = create_engine::lever::Lever {
                    state: state.clamp(0, 15) as u8,
                    due: (left > 0).then(|| now + i64::from(left)),
                };
                with_world(|w| w.dims[dim].levers.load(std::iter::once((pos, l))));
            }
        }
        Block::ContentObserver => {
            let o = Observer {
                filter: filter(OBSERVER),
                ..Observer::default()
            };
            with_world(|w| {
                w.dims[dim]
                    .detectors
                    .load(std::iter::once((pos, o)), std::iter::empty())
            });
        }
        Block::StockpileSwitch => {
            let v = ints(SWITCH);
            let [on_above, off_below, flags, level, min, max] = v[..] else {
                return;
            };
            let s = Switch {
                on_above,
                off_below,
                inverted: flags & 1 != 0,
                in_stacks: flags & 2 != 0,
                state: flags & 4 != 0,
                powered: flags & 8 != 0,
                filter: filter(SWITCH_FILTER),
                level,
                min,
                max,
            };
            with_world(|w| {
                w.dims[dim]
                    .detectors
                    .load(std::iter::empty(), std::iter::once((pos, s)))
            });
        }
        _ => {}
    }
}
