//! What logistics keeps for a block that rides a moving structure (`Subsystem::carry_out`,
//! Create's block entity NBT): the filter slot (`FilteringBehaviour`: `Filter`, `FilterAmount`,
//! `UpTo`) of a smart chute, a brass funnel, a basin, a saw or a deployer; a brass tunnel's side
//! filters (`SidedFilteringBehaviour`, by side: no `transform` turns them in Create either),
//! its selection mode and the stack it distributes.

use super::{brass_from_nbt, brass_to_nbt, tunnel_filters_from_nbt, tunnel_filters_to_nbt};
use crate::glue::items::filters;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use mod_api::{Compound, Tag};

const FILTER: &str = "Filter";
const AMOUNT: &str = "FilterAmount";
const SIDES: &str = "SideFilters";
const BRASS: &str = "BrassTunnel";

pub(super) fn carry_out(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block, out: &mut Compound) {
    let dim = lv.dim;
    let host = &*lv.host;
    let Some((filter, amount, sides, brass)) = with_world(|w| {
        let d = &mut w.dims[dim];
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

pub(super) fn carry_in(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block, data: &Compound) {
    let dim = lv.dim;
    let host = &*lv.host;
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
