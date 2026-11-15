//! What the machines keep for a block that rides a moving structure (`Subsystem::carry_out`,
//! Create's block entity NBT in `StructureBlockInfo.nbt`): a blaze burner's fuel and burn time
//! or creative heat (`BlazeBurnerBlockEntity.write`), a basin's tanks, a saw's inventory, a
//! crafter's slot cover and its joined inputs (`ConnectedInput`, turned with the structure as
//! `MechanicalCrafterBlockEntity.transform` turns them), and its phase, countdown and the grid
//! it holds mid-craft (`Phase`, `CountDown`, `GroupedItems`: they ride and go back rather than
//! drop). The keys are this module's.

use super::persist::{CREATIVE_BURNER, heat_of};
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::contraption::Transform;
use create_engine::{basin, burner, crafter, crafter_input, saw, tank};
use mod_api::{Compound, Tag};

const BURNER: &str = "Burner";
const BASIN: &str = "BasinTanks";
const SAW: &str = "SawItems";
const SAW_STATE: &str = "SawState";
const COVER: &str = "CrafterCovered";
const INPUT: &str = "CrafterInput";
const CRAFTING: &str = "CrafterPhase";
const GRID: &str = "CrafterGrid";

pub(super) fn carry_out(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block, out: &mut Compound) {
    let dim = lv.dim;
    with_world(|w| {
        let d = &mut w.dims[dim];
        match block {
            Block::BlazeBurner => {
                if let Some(b) = d.burners.remove(&pos) {
                    d.burning.retain(|p| *p != pos);
                    let v = match b.creative {
                        Some(h) => vec![CREATIVE_BURNER, h as i32],
                        None => vec![b.fuel.index(), b.remaining],
                    };
                    if b.saved() {
                        out.insert(BURNER.into(), Tag::IntArray(v));
                    }
                }
            }
            Block::Basin => {
                if let Some(t) = d.basin_fluids.remove(&pos) {
                    let mut codes = tank::Codes::default();
                    let v = t
                        .input
                        .iter()
                        .chain(&t.output)
                        .flat_map(|f| [codes.code(f.map(|f| f.0)), f.map_or(0, |f| f.1 as i32)])
                        .collect();
                    out.insert(BASIN.into(), Tag::IntArray(v));
                    crate::glue::fluids::put_fluid_keys(out, BASIN, codes);
                }
            }
            Block::MechanicalSaw => {
                if let Some(s) = d.saws.remove(&pos) {
                    let items = s
                        .slots
                        .iter()
                        .map(|(item, n)| {
                            Tag::Compound(Compound::from([
                                ("id".into(), Tag::String(item.as_str().into())),
                                ("count".into(), Tag::Int(i32::from(*n))),
                            ]))
                        })
                        .collect();
                    out.insert(SAW.into(), Tag::List(items));
                    out.insert(
                        SAW_STATE.into(),
                        Tag::IntArray(vec![
                            s.remaining.to_bits() as i32,
                            i32::from(s.applied),
                            s.index as i32,
                        ]),
                    );
                }
            }
            Block::MechanicalCrafter => {
                if d.crafter_covers.remove(&pos) {
                    out.insert(COVER.into(), Tag::Byte(1));
                }
                if let Some(i) = d.crafter_inputs.remove(&pos) {
                    let mut v = vec![i32::from(i.controller)];
                    v.extend(i.data.iter().flatten());
                    out.insert(INPUT.into(), Tag::IntArray(v));
                }
                // Taken before its block goes, so its grid rides rather than drops.
                if let Some(c) = d.crafters.remove(&pos) {
                    d.crafting.retain(|p| *p != pos);
                    if c != crafter::Crafter::default() {
                        out.insert(
                            CRAFTING.into(),
                            Tag::IntArray(vec![c.phase as i32, c.count_down]),
                        );
                        let grid = c.grid.iter().map(|((x, y), item)| {
                            Tag::Compound(Compound::from([
                                ("x".into(), Tag::Int(*x)),
                                ("y".into(), Tag::Int(*y)),
                                ("id".into(), Tag::String(item.as_str().into())),
                            ]))
                        });
                        out.insert(GRID.into(), Tag::List(grid.collect()));
                    }
                }
            }
            _ => {}
        }
    });
}

pub(super) fn carry_in(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    data: &Compound,
    turn: Transform,
) {
    let dim = lv.dim;
    let ints = |k: &str| match data.get(&k.into()) {
        Some(Tag::IntArray(v)) => Some(v.clone()),
        _ => None,
    };
    match block {
        Block::BlazeBurner => {
            let Some([kind, value]) = ints(BURNER).and_then(|v| <[i32; 2]>::try_from(v).ok())
            else {
                return;
            };
            let b = if kind == CREATIVE_BURNER {
                burner::Burner {
                    creative: Some(heat_of(value)),
                    ..burner::Burner::default()
                }
            } else {
                burner::Burner {
                    fuel: burner::Fuel::from_index(kind),
                    remaining: value,
                    creative: None,
                }
            };
            with_world(|w| w.load_burners(dim, std::iter::once((pos, b))));
        }
        Block::Basin => {
            let Some(v) = ints(BASIN).filter(|v| v.len() == 8) else {
                return;
            };
            let keys = crate::glue::fluids::fluid_keys(data, BASIN);
            let tank = |i: usize| {
                tank::Codes::fluid(&keys, v[2 * i]).map(|f| (f, v[2 * i + 1].clamp(0, 1000) as u32))
            };
            let t = basin::Tanks {
                input: [tank(0), tank(1)],
                output: [tank(2), tank(3)],
            };
            with_world(|w| {
                let d = &mut w.dims[dim];
                d.basin_fluids.insert(pos, t);
                d.basin_changes.push(pos);
            });
        }
        Block::MechanicalSaw => {
            let Some(Tag::List(items)) = data.get(&SAW.into()) else {
                return;
            };
            let slots = items
                .iter()
                .filter_map(|t| {
                    let Tag::Compound(c) = t else { return None };
                    let (Some(Tag::String(id)), Some(Tag::Int(n))) =
                        (c.get(&"id".into()), c.get(&"count".into()))
                    else {
                        return None;
                    };
                    Some((id.to_string_lossy(), (*n).clamp(1, 255) as u8))
                })
                .collect();
            let state = ints(SAW_STATE).unwrap_or_default();
            let s = saw::Saw {
                slots,
                remaining: state.first().map_or(0.0, |b| f32::from_bits(*b as u32)),
                applied: state.get(1).is_some_and(|v| *v != 0),
                index: state.get(2).map_or(0, |v| (*v).max(0) as usize),
                ..saw::Saw::default()
            };
            with_world(|w| w.dims[dim].saws.insert(pos, s));
        }
        Block::MechanicalCrafter => {
            let covered = data.contains_key(&COVER.into());
            let input = ints(INPUT).and_then(|v| {
                let (c, rest) = v.split_first()?;
                Some(crafter_input::Input {
                    controller: *c != 0,
                    data: rest
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|o| turn.vec(*o))
                        .collect(),
                })
            });
            let crafting = ints(CRAFTING).and_then(|v| {
                use crafter::Phase as P;
                let phase = [
                    P::Idle,
                    P::Accepting,
                    P::Assembling,
                    P::Exporting,
                    P::Waiting,
                    P::Crafting,
                    P::Inserting,
                ]
                .get(usize::try_from(*v.first()?).ok()?)
                .copied()?;
                let grid = match data.get(&GRID.into()) {
                    Some(Tag::List(l)) => l
                        .iter()
                        .filter_map(|t| {
                            let Tag::Compound(c) = t else { return None };
                            let (Some(Tag::Int(x)), Some(Tag::Int(y)), Some(Tag::String(id))) =
                                (c.get(&"x".into()), c.get(&"y".into()), c.get(&"id".into()))
                            else {
                                return None;
                            };
                            Some(((*x, *y), id.to_string_lossy()))
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                Some(crafter::Crafter {
                    phase,
                    count_down: v.get(1).copied().unwrap_or(0),
                    grid,
                })
            });
            with_world(|w| {
                let d = &mut w.dims[dim];
                if covered {
                    d.crafter_covers.insert(pos);
                }
                if let Some(i) = input {
                    d.crafter_inputs.insert(pos, i);
                }
                if let Some(c) = crafting {
                    // At work again where it went back (`tick` runs on from its phase).
                    if c.phase != crafter::Phase::Idle {
                        d.crafting.push(pos);
                    }
                    d.crafters.insert(pos, c);
                    d.crafter_checks.push(pos);
                }
            });
        }
        _ => {}
    }
}
