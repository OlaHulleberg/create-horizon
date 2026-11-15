//! What the fluid blocks keep for a block that rides a moving structure
//! (`Subsystem::carry_out`, Create's block entity NBT): a spout's tank, an item drain's tank and
//! held stack (`SmartFluidTankBehaviour`, `HeldItem`, `ProcessingTicks`), a smart fluid pipe's
//! filter (`FilteringBehaviour`) and a creative fluid tank's fluid. (A fluid tank's contents
//! ride with its tank: `contraptions::actors`.)

use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::fluids::SmartFilter;
use create_engine::kinetics::Dir;
use create_engine::tank::Fluid;
use mod_api::{Compound, Tag};

const SPOUT: &str = "SpoutTank";
const DRAIN: &str = "DrainTank";
const DRAIN_ITEM: &str = "DrainHeld";
const PIPE_FILTER: &str = "PipeFilter";
const CREATIVE: &str = "CreativeFluid";

fn tank_tag(f: Option<(Fluid, u32)>) -> Tag {
    let (fluid, n) = f.map_or((255, 0), |(f, n)| (i32::from(f.index()), n as i32));
    Tag::IntArray(vec![fluid, n])
}

fn tank_of(t: Option<&Tag>) -> Option<Option<(Fluid, u32)>> {
    match t {
        Some(Tag::IntArray(v)) if v.len() == 2 => {
            Some(Fluid::from_index(v[0].clamp(0, 255) as u8).map(|f| (f, v[1].max(0) as u32)))
        }
        _ => None,
    }
}

pub(super) fn carry_out(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block, out: &mut Compound) {
    let dim = lv.dim;
    with_world(|w| {
        let d = &mut w.dims[dim];
        match block {
            Block::Spout => {
                if let Some(s) = d.spouts.remove(&pos) {
                    out.insert(SPOUT.into(), tank_tag(s.fluid));
                }
            }
            Block::ItemDrain => {
                if let Some(dr) = d.drains.remove(&pos) {
                    out.insert(DRAIN.into(), tank_tag(dr.fluid));
                    if let Some(h) = dr.held {
                        out.insert(
                            DRAIN_ITEM.into(),
                            Tag::Compound(Compound::from([
                                ("Item".into(), Tag::String(h.item.as_str().into())),
                                ("Count".into(), Tag::Int(i32::from(h.count))),
                                ("BeltPosition".into(), Tag::Float(h.position.to_bits())),
                                ("From".into(), Tag::Int(h.from as i32)),
                                ("ProcessingTicks".into(), Tag::Int(dr.processing)),
                            ])),
                        );
                    }
                }
            }
            Block::SmartFluidPipe => {
                if let Some(f) = d.plumbing.filters.remove(&pos) {
                    let fluid = f.fluid.map_or(255, |f| i32::from(f.index()));
                    out.insert(
                        PIPE_FILTER.into(),
                        Tag::List(vec![Tag::String(f.item.as_str().into()), Tag::Int(fluid)]),
                    );
                }
            }
            Block::CreativeFluidTank => {
                if let Some(Some(f)) = d.plumbing.creative.get(&pos).copied() {
                    out.insert(CREATIVE.into(), Tag::Int(i32::from(f.index())));
                }
            }
            _ => {}
        }
    });
}

pub(super) fn carry_in(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block, data: &Compound) {
    let dim = lv.dim;
    match block {
        Block::Spout => {
            if let Some(fluid) = tank_of(data.get(&SPOUT.into())) {
                with_world(|w| w.dims[dim].spouts.entry(pos).or_default().fluid = fluid);
            }
        }
        Block::ItemDrain => {
            let Some(fluid) = tank_of(data.get(&DRAIN.into())) else {
                return;
            };
            let held = match data.get(&DRAIN_ITEM.into()) {
                Some(Tag::Compound(c)) => {
                    let get = |k: &str| c.get(&k.into());
                    let int = |k: &str| match get(k) {
                        Some(Tag::Int(v)) => *v,
                        _ => 0,
                    };
                    match get("Item") {
                        Some(Tag::String(item)) => {
                            let position = match get("BeltPosition") {
                                Some(Tag::Float(b)) => f32::from_bits(*b),
                                _ => 0.0,
                            };
                            Some((
                                create_engine::drain::Held {
                                    item: item.to_string_lossy(),
                                    count: int("Count").clamp(1, 255) as u8,
                                    position,
                                    prev: position,
                                    from: Dir::ALL[int("From").clamp(0, 5) as usize],
                                },
                                int("ProcessingTicks"),
                            ))
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            with_world(|w| {
                let dr = w.dims[dim].drains.entry(pos).or_default();
                dr.fluid = fluid;
                if let Some((h, processing)) = held {
                    dr.held = Some(h);
                    dr.processing = processing;
                }
            });
        }
        Block::SmartFluidPipe => {
            let Some(Tag::List(v)) = data.get(&PIPE_FILTER.into()) else {
                return;
            };
            let (Some(Tag::String(item)), Some(Tag::Int(f))) = (v.first(), v.get(1)) else {
                return;
            };
            let filter = SmartFilter {
                item: item.to_string_lossy(),
                fluid: Fluid::from_index((*f).clamp(0, 255) as u8),
            };
            with_world(|w| {
                let p = &mut w.dims[dim].plumbing;
                p.filters.insert(pos, filter);
                p.filter_changes.push(pos);
            });
        }
        Block::CreativeFluidTank => {
            if let Some(Tag::Int(f)) = data.get(&CREATIVE.into()) {
                let fluid = Fluid::from_index((*f).clamp(0, 255) as u8);
                with_world(|w| w.dims[dim].plumbing.creative_saved.insert(pos, fluid));
            }
        }
        _ => {}
    }
}
