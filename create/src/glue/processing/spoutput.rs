//! A basin's spout (`BasinBlockEntity.updateSpoutput`, `acceptOutputs`,
//! `tryClearingSpoutputOverflow`): a basin turns its `facing` toward a belt, depot, chute,
//! upward funnel or processing machine diagonally below it, and the item results of its recipes
//! go there through its spout (a buffer it empties each tick) instead of into its outputs. The
//! wrench chooses or shuts a side (`onWrenched`). The items going out are drawn falling from the
//! spout for a moment (`visualizedOutputItems`).
//!
//! Its fluid results go out the same way into what holds fluids there (an item drain or a
//! basin, force-filled whole or not at all; `spoutputFluidBuffer`); with nothing there that
//! holds fluids they stay in its own output tank, as Create's do. The fluids leaving the spout
//! are not drawn (`visualizedOutputFluids`).

use super::stacks;
use crate::glue::logistics::{self, items};
use crate::glue::server::{ServerLevel, update_all};
use crate::glue::world::with_world;
use create_engine::basin;
use create_engine::kinetics::Dir;
use create_engine::processing::Slot;
use create_engine::tank::{Codes, Fluid};
use mod_api::{Compound, Tag};
use std::cell::RefCell;
use std::collections::HashMap;

/// Where a basin keeps the results waiting to go out (`Overflow`), by id and count.
const OVERFLOW_KEY: &str = "Overflow";
/// Where it keeps the fluid results waiting to go out (`FluidOverflow`): fluid (`tank::Codes`,
/// its keys beside it) and amount, in turn.
const FLUID_OVERFLOW_KEY: &str = "FluidOverflow";
/// The side the wrench chose (`PreferredSpoutput`).
const PREFERRED_KEY: &str = "PreferredSpoutput";
/// The sides the wrench shut (`DisabledSpoutput`).
const DISABLED_KEY: &str = "DisabledSpoutput";
/// The key of a basin's block entity data its clients draw the items leaving its spout from
/// (`VisualizedItems`): each one's `id` and `S`, the game time it left.
pub const VISUALIZED_KEY: &str = "VisualizedItems";
/// `OUTPUT_ANIMATION_TIME`: ticks an item is seen leaving the spout.
pub const OUTPUT_ANIMATION_TIME: i64 = 10;
/// `SmartBlockEntity`'s lazy tick: every 10 ticks.
const LAZY_RATE: i64 = 10;

thread_local! {
    /// Basins with results waiting to go out, or items just seen leaving, by dimension.
    static BUSY: RefCell<HashMap<(usize, [i32; 3]), ()>> = RefCell::new(HashMap::new());
}

/// Forgets what was kept for the world.
pub fn clear() {
    BUSY.with_borrow_mut(HashMap::clear);
}

/// `Iterate.horizontalDirections`: south, west, north, east.
const HORIZONTAL: [Dir; 4] = [Dir::South, Dir::West, Dir::North, Dir::East];

fn dir_name(d: Dir) -> &'static str {
    crate::glue::kinetics::dir_name(d)
}

fn dir_of(name: &str) -> Option<Dir> {
    Dir::ALL.into_iter().find(|&d| dir_name(d) == name)
}

/// The basin's facing: down, or the side its spout points to.
pub fn facing(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Dir {
    lv.property(pos, "facing")
        .and_then(|f| dir_of(&f))
        .filter(|d| *d != Dir::Up)
        .unwrap_or(Dir::Down)
}

fn data(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Compound> {
    lv.entity_at(pos)?.data(&*lv.host)
}

fn below_side(pos: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [pos[0] + o[0], pos[1] - 1, pos[2] + o[2]]
}

/// `BasinBlock.canOutputTo`: a block of air (or a funnel not facing that way) beside the basin,
/// and below that a belt not running back toward it or something else taking items from that
/// side (`DirectBeltInputBehaviour`).
fn can_output_to(lv: &ServerLevel<'_>, pos: [i32; 3], d: Dir) -> bool {
    let o = d.offset();
    let neighbour = [pos[0] + o[0], pos[1], pos[2] + o[2]];
    let output = below_side(pos, d);
    if let Some(f) = logistics::funnel_at(lv, neighbour) {
        if f.facing == d {
            return false;
        }
    } else if lv.blocked(neighbour) {
        return false;
    }
    logistics::direct_input_at(lv, output) && logistics::direct_input_accepts(lv, output, d)
}

/// Whether something below the side `d` takes the basin's items (`acceptOutputs`' `targetInv`:
/// the item handler there, or the inventory a funnel there feeds): a belt, a depot, a chute, a
/// machine or any inventory.
fn has_target(lv: &mut ServerLevel<'_>, pos: [i32; 3], d: Dir) -> bool {
    let output = below_side(pos, d);
    if logistics::belt_segment(lv, output).is_some() || super::belt_input_side(lv, output).is_some()
    {
        return true;
    }
    if let Some(f) = logistics::funnel_at(lv, output) {
        let behind = create_engine::funnel::behind(output, f.facing);
        return items::items_at(lv, behind, Some(items::face(f.facing))).is_some();
    }
    items::items_at(lv, output, Some(items::face(d.opposite()))).is_some()
}

/// Puts what it can of `count` of `item` into the target below side `d`; how many it took.
/// `simulate`: only how many it would.
fn insert(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    d: Dir,
    item: &str,
    count: u8,
    simulate: bool,
) -> u8 {
    let output = below_side(pos, d);
    if let Some(left) = logistics::put_on_from_above(lv, output, item, count, None, simulate) {
        return count - left;
    }
    if super::belt_input_side(lv, output).is_some() {
        if simulate {
            // Machines take while they have room; trying is the only way to know.
            return count;
        }
        let up = logistics::upright(&*lv.host, item);
        let one = create_engine::belt::Item::new(item, count, 0, up);
        let left = super::belt_input(lv, output, d, &one).unwrap_or(count);
        return count - left;
    }
    let Some(stack) = stacks::stack(&*lv.host, item, u32::from(count)) else {
        return 0;
    };
    let (at, side) = match logistics::funnel_at(lv, output) {
        Some(f) => {
            // `filter.test`: a brass funnel's filter.
            if !logistics::funnel_passes(lv, output, &stack) {
                return 0;
            }
            (create_engine::funnel::behind(output, f.facing), f.facing)
        }
        None => (output, d.opposite()),
    };
    if simulate {
        let room = items::insert_room(lv, at, items::face(side), &stack).unwrap_or(0);
        return room.min(u32::from(count)) as u8;
    }
    let left = items::insert_item(lv, at, items::face(side), stack).map_or(0, |s| s.count());
    count - left.min(u32::from(count)) as u8
}

fn overflow(data: &Compound) -> Vec<(String, u8)> {
    let Some(Tag::List(list)) = data.get(&OVERFLOW_KEY.into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let Some(Tag::String(id)) = c.get(&"id".into()) else {
                return None;
            };
            let Some(Tag::Int(n)) = c.get(&"count".into()) else {
                return None;
            };
            Some((id.to_string().ok()?, (*n).clamp(0, 255) as u8))
        })
        .collect()
}

fn overflow_tag(list: &[(String, u8)]) -> Tag {
    Tag::List(
        list.iter()
            .map(|(id, n)| {
                Tag::Compound(Compound::from([
                    ("id".into(), Tag::String(id.as_str().into())),
                    ("count".into(), Tag::Int(i32::from(*n))),
                ]))
            })
            .collect(),
    )
}

/// `canContinueProcessing`: nothing waits to go out of the spout.
pub fn can_continue(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    data(lv, pos).is_none_or(|d| overflow(&d).is_empty() && fluid_overflow(&d).is_empty())
}

/// What holds fluids below the side `d` (`FluidHandler.BLOCK` there): an item drain or a basin.
fn fluid_target(lv: &ServerLevel<'_>, pos: [i32; 3], d: Dir) -> Option<FluidTarget> {
    let output = below_side(pos, d);
    let dim = lv.dim;
    if with_world(|w| w.dims[dim].drains.contains_key(&output)) == Some(true) {
        return Some(FluidTarget::Drain(output));
    }
    super::basin::basin_at(lv, output).then_some(FluidTarget::Basin(output))
}

#[derive(Clone, Copy)]
enum FluidTarget {
    Drain([i32; 3]),
    Basin([i32; 3]),
}

/// `forceFill` of `amount` of `fluid` into `t`, all of it or none: whether it went in.
fn fill_whole(lv: &ServerLevel<'_>, t: FluidTarget, fluid: Fluid, amount: u32) -> bool {
    let dim = lv.dim;
    match t {
        FluidTarget::Drain(p) => with_world(|w| {
            let d = w.dims[dim].drains.get_mut(&p)?;
            let next = match d.fluid {
                None => amount,
                Some((f, n)) if f == fluid => n + amount,
                _ => return Some(false),
            };
            if next > create_engine::drain::CAPACITY {
                return Some(false);
            }
            d.fluid = Some((fluid, next));
            w.dims[dim].analog_checks.push(p);
            w.mark_unsaved(dim, p);
            Some(true)
        })
        .flatten()
        .unwrap_or(false),
        FluidTarget::Basin(p) => {
            // Into its inputs (its outputs forbid insertion).
            let mut tanks = super::basin::basin_tanks(dim, p);
            let mut input = tanks.input;
            if basin::fill(&mut input, fluid, amount) != amount {
                return false;
            }
            tanks.input = input;
            with_world(|w| w.set_basin_fluids(dim, p, tanks));
            true
        }
    }
}

fn fluid_overflow(data: &Compound) -> Vec<(Fluid, u32)> {
    let Some(Tag::IntArray(v)) = data.get(&FLUID_OVERFLOW_KEY.into()) else {
        return Vec::new();
    };
    let keys = crate::glue::fluids::fluid_keys(data, FLUID_OVERFLOW_KEY);
    v.as_chunks::<2>()
        .0
        .iter()
        .filter_map(|c| Some((Codes::fluid(&keys, c[0])?, c[1].max(0) as u32)))
        .collect()
}

/// The fluid buffer's values to write: the array and the keys beside it.
fn fluid_overflow_values(list: &[(Fluid, u32)]) -> Vec<(String, Option<Tag>)> {
    let mut codes = Codes::default();
    let v = list
        .iter()
        .flat_map(|(f, n)| [codes.code(Some(*f)), *n as i32])
        .collect();
    let mut c = Compound::new();
    crate::glue::fluids::put_fluid_keys(&mut c, FLUID_OVERFLOW_KEY, codes);
    let keys_key = format!("{FLUID_OVERFLOW_KEY}_fluids");
    let keys = c.get(&keys_key.as_str().into()).cloned();
    vec![
        (FLUID_OVERFLOW_KEY.to_owned(), Some(Tag::IntArray(v))),
        (keys_key, keys),
    ]
}

fn set_values(lv: &mut ServerLevel<'_>, pos: [i32; 3], values: &[(String, Option<Tag>)]) {
    let refs: Vec<(&str, Option<Tag>)> = values
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    super::basin::set_shown(lv, pos, &refs);
}

/// `acceptOutputs` for a recipe's fluid results, after it put them into `after`'s output tanks:
/// facing a side with something holding fluids below it, what it added goes into the spout's
/// fluid buffer instead (`spoutputFluidBuffer`); else it stays in the basin.
pub fn route_fluids(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    before: &basin::Tanks,
    after: &mut basin::Tanks,
) {
    let d = facing(lv, pos);
    if d == Dir::Down || fluid_target(lv, pos, d).is_none() {
        return;
    }
    let mut added: Vec<(Fluid, u32)> = Vec::new();
    for i in 0..after.output.len() {
        let Some((f, n)) = after.output[i] else {
            continue;
        };
        let had = before.output[i]
            .filter(|(g, _)| *g == f)
            .map_or(0, |(_, m)| m);
        if n > had {
            added.push((f, n - had));
            after.output[i] = (had > 0).then_some((f, had));
        }
    }
    if added.is_empty() {
        return;
    }
    let mut list = data(lv, pos)
        .map(|d| fluid_overflow(&d))
        .unwrap_or_default();
    list.extend(added);
    let values = fluid_overflow_values(&list);
    set_values(lv, pos, &values);
    BUSY.with_borrow_mut(|b| b.insert((lv.dim, pos), ()));
}

/// `acceptOutputs(.., simulate)` for a recipe with item results: facing a side, something
/// there must take items.
pub fn accepts(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> bool {
    let d = facing(lv, pos);
    d == Dir::Down || has_target(lv, pos, d)
}

/// `acceptOutputs` after a recipe put its item results into `after`'s outputs: facing a side,
/// what it added there goes into the spout's buffer instead.
pub fn route(lv: &mut ServerLevel<'_>, pos: [i32; 3], before: &[Slot], after: &mut [Slot]) {
    if facing(lv, pos) == Dir::Down {
        return;
    }
    let mut added: Vec<(String, u8)> = Vec::new();
    for i in basin::OUTPUTS {
        let was = before.get(i).cloned().flatten();
        let now = after.get(i).cloned().flatten();
        let Some((id, n)) = now else { continue };
        let had = was.filter(|(w, _)| *w == id).map_or(0, |(_, m)| m);
        if n > had {
            match added.iter_mut().find(|(a, _)| *a == id) {
                Some((_, m)) => *m = m.saturating_add(n - had),
                None => added.push((id.clone(), n - had)),
            }
            after[i] = (had > 0).then_some((id, had));
        }
    }
    if added.is_empty() {
        return;
    }
    let mut list = data(lv, pos).map(|d| overflow(&d)).unwrap_or_default();
    list.extend(added);
    super::basin::set_shown(lv, pos, &[(OVERFLOW_KEY, Some(overflow_tag(&list)))]);
    BUSY.with_borrow_mut(|b| b.insert((lv.dim, pos), ()));
}

/// The wrench on a basin's `face` (`onWrenched`): the side it faces is shut (and no longer
/// chosen); another side is chosen. Then it looks again where to put its results.
pub fn wrenched(lv: &mut ServerLevel<'_>, pos: [i32; 3], face: Dir) {
    let Some(d) = data(lv, pos) else { return };
    let current = facing(lv, pos);
    let mut disabled = disabled_of(&d);
    let mut preferred = preferred_of(&d);
    disabled.retain(|x| *x != face);
    if current == face {
        if preferred == Some(face) {
            preferred = None;
        }
        disabled.push(face);
    } else {
        preferred = Some(face);
    }
    let disabled_tag = Tag::List(
        disabled
            .iter()
            .map(|x| Tag::String(dir_name(*x).to_uppercase().as_str().into()))
            .collect(),
    );
    let preferred_tag = Tag::String(
        preferred
            .map_or("", dir_name)
            .to_uppercase()
            .as_str()
            .into(),
    );
    super::basin::set_shown(
        lv,
        pos,
        &[
            (DISABLED_KEY, Some(disabled_tag)),
            (PREFERRED_KEY, Some(preferred_tag)),
        ],
    );
    update(lv, pos);
}

fn disabled_of(d: &Compound) -> Vec<Dir> {
    let Some(Tag::List(list)) = d.get(&DISABLED_KEY.into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| match t {
            Tag::String(s) => dir_of(&s.to_string().ok()?.to_lowercase()),
            _ => None,
        })
        .collect()
}

fn preferred_of(d: &Compound) -> Option<Dir> {
    match d.get(&PREFERRED_KEY.into()) {
        Some(Tag::String(s)) => dir_of(&s.to_string().ok()?.to_lowercase()),
        _ => None,
    }
}

/// `updateSpoutput`: faces the last side it may output to and isn't shut (the chosen one if it
/// may), else down; turned to a side, what its outputs hold goes out through the spout.
fn update(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let current = facing(lv, pos);
    let d = data(lv, pos).unwrap_or_default();
    let disabled = disabled_of(&d);
    let mut next = Dir::Down;
    for test in HORIZONTAL {
        if can_output_to(lv, pos, test) && !disabled.contains(&test) {
            next = test;
        }
    }
    if let Some(p) = preferred_of(&d)
        && p != Dir::Up
        && can_output_to(lv, pos, p)
    {
        next = p;
    }
    if next == current {
        return;
    }
    lv.set_property(pos, "facing", dir_name(next), update_all());
    if next == Dir::Down || !has_target(lv, pos, next) {
        return;
    }
    let Some(before) = lv.read_slots(pos, 0, basin::SLOTS) else {
        return;
    };
    let mut after = before.clone();
    let mut list = overflow(&d);
    for i in basin::OUTPUTS {
        if let Some((id, n)) = after[i].take() {
            list.push((id, n));
        }
    }
    if after != before {
        let changes: Vec<(usize, Option<mod_api::ItemStack>)> = basin::OUTPUTS
            .filter(|&i| before[i].is_some())
            .map(|i| (i, None))
            .collect();
        lv.set_slots(pos, &changes);
        super::basin::basin_changed(lv.dim, pos);
        super::basin::set_shown(lv, pos, &[(OVERFLOW_KEY, Some(overflow_tag(&list)))]);
        BUSY.with_borrow_mut(|b| b.insert((lv.dim, pos), ()));
    }
}

/// Each tick: every lazy tick the loaded basins look where to put their results; those with
/// results waiting put them out (`tryClearingSpoutputOverflow`), and the items seen leaving are
/// let go after their moment.
pub fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now = lv.game_time();
    if now % LAZY_RATE == 0 {
        let basins: Vec<[i32; 3]> =
            with_world(|w| w.dims[dim].basins.iter().copied().collect()).unwrap_or_default();
        for pos in basins {
            if !lv.is_loaded(pos) || !super::basin::basin_at(lv, pos) {
                continue;
            }
            update(lv, pos);
            // A buffer read back from a save.
            if !can_continue(lv, pos) {
                BUSY.with_borrow_mut(|b| b.insert((dim, pos), ()));
            }
        }
    }
    let busy: Vec<[i32; 3]> = BUSY.with_borrow(|b| {
        b.keys()
            .filter(|(d, _)| *d == dim)
            .map(|(_, p)| *p)
            .collect()
    });
    for pos in busy {
        if !lv.is_loaded(pos) || !super::basin::basin_at(lv, pos) {
            BUSY.with_borrow_mut(|b| b.remove(&(dim, pos)));
            continue;
        }
        if !clear_overflow(lv, pos, now) {
            BUSY.with_borrow_mut(|b| b.remove(&(dim, pos)));
        }
    }
}

/// `tryClearingSpoutputOverflow`: each waiting stack into the target as far as it takes it
/// (facing down they pop out); then the items seen leaving that are done are let go. Whether
/// the basin is still busy.
fn clear_overflow(lv: &mut ServerLevel<'_>, pos: [i32; 3], now: i64) -> bool {
    let Some(d) = data(lv, pos) else {
        return false;
    };
    let mut list = overflow(&d);
    let mut seen = visualized(&d);
    let seen_before = seen.clone();
    let before = list.clone();
    let facing = facing(lv, pos);
    let mut i = 0;
    while i < list.len() {
        let (item, n) = list[i].clone();
        if facing == Dir::Down {
            let at = pos.map(|v| f64::from(v) + 0.5);
            lv.spawn_moving(at, &item, u32::from(n), [0.0; 3]);
            list.remove(i);
            continue;
        }
        if !has_target(lv, pos, facing) {
            break;
        }
        let would = insert(lv, pos, facing, &item, n, true);
        if would == 0 {
            i += 1;
            continue;
        }
        if seen.len() < 3 {
            seen.push((item.clone(), now));
        }
        let took = insert(lv, pos, facing, &item, n, false);
        if took >= n {
            list.remove(i);
        } else {
            list[i].1 = n - took;
            i += 1;
        }
    }
    // The fluids: each into what holds fluids there whole, or it waits (facing down, gone).
    let fluids = fluid_overflow(&d);
    let mut fluids_left = fluids.clone();
    if !fluids.is_empty() {
        let target = (facing != Dir::Down)
            .then(|| fluid_target(lv, pos, facing))
            .flatten();
        fluids_left.retain(|&(f, n)| {
            if facing == Dir::Down {
                return false;
            }
            // Nothing holding fluids there: they wait (`targetTank == null`).
            target.is_none_or(|t| !fill_whole(lv, t, f, n))
        });
    }
    if fluids_left != fluids {
        let values = fluid_overflow_values(&fluids_left);
        set_values(lv, pos, &values);
        if list.is_empty() && fluids_left.is_empty() {
            with_world(|w| {
                w.dims[lv.dim]
                    .mixer_checks
                    .push([pos[0], pos[1] + 2, pos[2]])
            });
        }
    }
    seen.retain(|(_, at)| now - at < OUTPUT_ANIMATION_TIME);
    let mut values = Vec::new();
    if list != before {
        values.push((OVERFLOW_KEY, Some(overflow_tag(&list))));
        if list.is_empty() && fluids_left.is_empty() {
            // `notifyChangeOfContents`: its mixer or press may go on.
            with_world(|w| {
                w.dims[lv.dim]
                    .mixer_checks
                    .push([pos[0], pos[1] + 2, pos[2]])
            });
        }
    }
    if seen != seen_before {
        let tag = Tag::List(
            seen.iter()
                .map(|(id, at)| {
                    Tag::Compound(Compound::from([
                        ("id".into(), Tag::String(id.as_str().into())),
                        ("S".into(), Tag::Long(*at)),
                    ]))
                })
                .collect(),
        );
        values.push((VISUALIZED_KEY, Some(tag)));
    }
    if !values.is_empty() {
        super::basin::set_shown(lv, pos, &values);
    }
    !list.is_empty() || !fluids_left.is_empty() || !seen.is_empty()
}

/// The items seen leaving a basin's spout: id and the game time they left.
pub fn visualized(d: &Compound) -> Vec<(String, i64)> {
    let Some(Tag::List(list)) = d.get(&VISUALIZED_KEY.into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let Some(Tag::String(id)) = c.get(&"id".into()) else {
                return None;
            };
            let Some(Tag::Long(at)) = c.get(&"S".into()) else {
                return None;
            };
            Some((id.to_string().ok()?, *at))
        })
        .collect()
}
