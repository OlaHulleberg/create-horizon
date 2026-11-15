//! What logistics keeps for a block that rides a moving structure (`Subsystem::carry_out`,
//! Create's block entity NBT): the filter slot (`FilteringBehaviour`: `Filter`, `FilterAmount`,
//! `UpTo`) of a smart chute, a brass funnel, a basin, a saw or a deployer; a brass tunnel's side
//! filters (`SidedFilteringBehaviour`, by side: no `transform` turns them in Create either),
//! its selection mode and the stack it distributes; a belt's items (`BeltBlockEntity.write`'s
//! `Inventory`: the whole belt moves with any of its segments, `Contraption.moveBelt`), kept by
//! the first of its segments taken, with where its controller was from there.

use super::{brass_from_nbt, brass_to_nbt, tunnel_filters_from_nbt, tunnel_filters_to_nbt};
use crate::glue::items::filters;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use mod_api::{Compound, Tag};

pub(crate) const FILTER: &str = "Filter";
const AMOUNT: &str = "FilterAmount";
const SIDES: &str = "SideFilters";
const BRASS: &str = "BrassTunnel";
/// The angle a depot's held stack lies at (`HeldItem`'s `Angle`), for drawing it moving.
pub(crate) const DEPOT_ANGLE: &str = "DepotAngle";
/// A belt segment riding (so its renderer draws it: `BeltRenderer` in `renderBlockEntities`).
pub(super) const BELT_SEGMENT: &str = "BeltSegment";
/// The belt's items, as a column saves them, its controller at `pos` from the segment.
const BELT_ITEMS: &str = "BeltItems";
/// The belt as its clients draw it (`belt::encode`), its controller as an offset from the
/// segment, for drawing its items moving.
pub(super) const BELT_SHOWN: &str = "BeltShown";

pub(super) fn carry_out(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block, out: &mut Compound) {
    let dim = lv.dim;
    let host = &*lv.host;
    if block == Block::Belt {
        carry_belt(dim, pos, out);
    }
    let Some((filter, amount, sides, brass)) = with_world(|w| {
        let d = &mut w.dims[dim];
        if let Some(Some(h)) = d.depots.get(&pos) {
            out.insert(DEPOT_ANGLE.into(), Tag::Int(h.angle));
        }
        let filter = d.chute_filters.remove(&pos);
        let amount = d.filter_amounts.remove(&pos);
        let sides: Vec<_> = create_engine::kinetics::Dir::ALL
            .into_iter()
            .filter_map(|s| Some(((pos, s), d.tunnel_filters.remove(&(pos, s))?)))
            .collect();
        let brass = (block == Block::BrassTunnel)
            .then(|| {
                let b = d.brass.remove(&pos)?;
                d.brass_running.retain(|p| *p != pos);
                Some(b)
            })
            .flatten();
        (filter, amount, sides, brass)
    }) else {
        return;
    };
    if let Some(t) = filter
        .as_ref()
        .and_then(|f| filters::filter_to_nbt(host, f))
    {
        out.insert(FILTER.into(), t);
    }
    if let Some(a) = amount {
        out.insert(
            AMOUNT.into(),
            Tag::IntArray(vec![i32::from(a.count), i32::from(a.up_to)]),
        );
    }
    if !sides.is_empty() {
        let mut c = Compound::new();
        tunnel_filters_to_nbt(host, &mut c, &sides);
        out.insert(SIDES.into(), Tag::Compound(c));
    }
    if let Some(b) = brass {
        out.insert(BRASS.into(), brass_to_nbt(&[([0; 3], b)]));
    }
}

pub(super) fn carry_in(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    data: &Compound,
    turn: create_engine::contraption::Transform,
) {
    let dim = lv.dim;
    let host = &*lv.host;
    if block == Block::Belt
        && let Some(Tag::List(list)) = data.get(&BELT_ITEMS.into())
    {
        // Its chain forms again from the segments' states once they are back, and its
        // controller (where the turned offset points) takes the items.
        let saved = Compound::from([("belts".into(), Tag::List(list.clone()))]);
        for (offset, items) in super::belts_from_nbt(&saved) {
            let controller = add(pos, turn.vec(offset));
            with_world(|w| w.reform_belt(dim, controller, &[pos], items));
        }
    }
    let filter = data
        .get(&FILTER.into())
        .and_then(|t| filters::filter_from_nbt(host, t));
    let amount = match data.get(&AMOUNT.into()) {
        Some(Tag::IntArray(v)) if v.len() == 2 => Some(create_engine::board::FilterAmount::new(
            v[0].clamp(0, 255) as u8,
            v[1] != 0,
        )),
        _ => None,
    };
    let sides = match data.get(&SIDES.into()) {
        Some(Tag::Compound(c)) => tunnel_filters_from_nbt(host, c),
        _ => Vec::new(),
    };
    let brass = match data.get(&BRASS.into()) {
        Some(t) if block == Block::BrassTunnel => {
            brass_from_nbt(&Compound::from([("brass".into(), t.clone())]))
                .into_iter()
                .next()
                .map(|(_, b)| b)
        }
        _ => None,
    };
    with_world(|w| {
        let d = &mut w.dims[dim];
        if let Some(f) = filter {
            d.chute_filters.insert(pos, f);
        }
        if let Some(a) = amount {
            d.filter_amounts.insert(pos, a);
        }
        for ((_, side), f) in sides {
            d.tunnel_filters.insert((pos, side), f);
        }
        if let Some(b) = brass {
            w.load_brass(dim, std::iter::once((pos, b)));
        }
    });
}

fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// A belt segment leaving with a moving structure: the whole belt leaves its tables the first
/// time (`take_belt`), quietly (no segment breaks into a shaft, nothing drops), its items kept by
/// this segment; each segment is marked as a belt for its drawing.
fn carry_belt(dim: usize, pos: [i32; 3], out: &mut Compound) {
    out.insert(BELT_SEGMENT.into(), Tag::Byte(1));
    let taken = with_world(|w| {
        w.remove_belt_quietly(dim, pos);
        let (controller, belt) = w.take_belt(dim, pos)?;
        for p in &belt.chain {
            w.remove_belt_quietly(dim, *p);
        }
        Some((controller, belt))
    })
    .flatten();
    let Some((controller, belt)) = taken else {
        return;
    };
    let offset = [0, 1, 2].map(|i| controller[i] - pos[i]);
    out.insert(
        BELT_ITEMS.into(),
        super::belts_to_nbt(&[(offset, belt.items.to_save())]),
    );
    let shown = create_engine::belt::Contents {
        controller: offset,
        segment: belt.segment,
        length: belt.chain.len() as u16,
        ending: create_engine::belt::Ending::Eject,
        items: belt.items,
    };
    if let Some(bytes) = create_engine::belt::encode(&[shown]).into_iter().next() {
        out.insert(BELT_SHOWN.into(), Tag::ByteArray(bytes));
    }
}
