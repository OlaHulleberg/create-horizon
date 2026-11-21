//! The machines' state, saved in their block entities: deployers, crushing wheel controllers,
//! blaze burners, basins' tanks and mechanical crafters' connections and covers. An older
//! version kept them with the column under `deployers`, `crushers`, `burners`, `basins`,
//! `crafter_inputs` and `crafter_covers`, read once by [`load`].

use crate::glue::entity_data::Entities;
use crate::glue::world::with_world;
use create_engine::{Block, basin, burner, crafter_input, crusher, deployer, tank};
use mod_api::{Compound, Tag};

/// What an older version saved with the column, handed to the engine before the column's blocks
/// are found.
pub fn load(dim: usize, data: &Compound) {
    let deployers = deployers_from_nbt(data);
    let crushers = crushers_from_nbt(data);
    let burners = burners_from_nbt(data);
    let basins = basins_from_nbt(data);
    let inputs = inputs_from_nbt(data);
    let covers = covers_from_nbt(data);
    with_world(|w| {
        w.load_deployers(dim, deployers.into_iter());
        w.load_crushers(dim, crushers.into_iter());
        w.load_burners(dim, burners.into_iter());
        let d = &mut w.dims[dim];
        d.crafter_inputs.extend(inputs);
        d.crafter_covers.extend(covers);
        d.basin_fluids.extend(basins);
    });
}

const DEPLOYER: &str = "DeployerState";
const CRUSHER: &str = "CrusherState";
const BURNER: &str = "BurnerState";
const BASIN: &str = "BasinTanks";
const INPUT: &str = "ConnectedInput";
const COVER: &str = "Cover";

pub(super) fn saved_keys(block: Block) -> &'static [&'static str] {
    match block {
        Block::Deployer => &[DEPLOYER],
        Block::CrushingWheelController => &[CRUSHER],
        Block::BlazeBurner => &[BURNER],
        Block::Basin => &[BASIN],
        Block::MechanicalCrafter => &[INPUT, COVER],
        _ => &[],
    }
}

fn int(c: &Compound, k: &str) -> i32 {
    match c.get(&k.into()) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    }
}

fn fluid_tag(f: Option<(tank::Fluid, u32)>) -> Tag {
    let mut c = Compound::new();
    if let Some((fluid, n)) = f {
        c.insert("FluidName".into(), Tag::String(fluid.key().as_str().into()));
        c.insert("Amount".into(), Tag::Int(n as i32));
    }
    Tag::Compound(c)
}

fn fluid_of(t: &Tag) -> Option<(tank::Fluid, u32)> {
    let Tag::Compound(c) = t else { return None };
    let Some(Tag::String(name)) = c.get(&"FluidName".into()) else {
        return None;
    };
    let fluid = tank::Fluid::of_key(&name.to_string_lossy())?;
    Some((fluid, int(c, "Amount").clamp(0, 1000) as u32))
}

/// `DeployerBlockEntity.write`, `CrushingWheelControllerBlockEntity.write`,
/// `BlazeBurnerBlockEntity.write`, `BasinBlockEntity.write`'s tanks and
/// `MechanicalCrafterBlockEntity.write`'s `ConnectedInput` and `Cover` of column `chunk`.
pub fn save_entities(dim: usize, chunk: [i32; 2], out: &mut Entities) {
    with_world(|w| {
        let (cx, cz) = (chunk[0], chunk[1]);
        let mine = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
        for (p, d) in w.column_deployers(dim, cx, cz) {
            out.entry(p)
                .or_default()
                .insert(DEPLOYER.into(), Tag::Compound(deployer_to_tag(&d)));
        }
        for (p, c) in w.column_crushers(dim, cx, cz) {
            let items = c
                .slots
                .iter()
                .map(|(id, n)| {
                    Tag::Compound(Compound::from([
                        ("id".into(), Tag::String(id.as_str().into())),
                        ("count".into(), Tag::Int(i32::from(*n))),
                    ]))
                })
                .collect();
            let tag = Compound::from([
                ("Remaining".into(), Tag::Float(c.remaining.to_bits())),
                ("Applied".into(), Tag::Byte(i8::from(c.applied))),
                ("Items".into(), Tag::List(items)),
            ]);
            out.entry(p)
                .or_default()
                .insert(CRUSHER.into(), Tag::Compound(tag));
        }
        for (p, b) in w.column_burners(dim, cx, cz) {
            let (fuel, remaining) = match b.creative {
                Some(h) => (CREATIVE_BURNER, h as i32),
                None => (b.fuel.index(), b.remaining),
            };
            let tag = Compound::from([
                ("Fuel".into(), Tag::Int(fuel)),
                ("Remaining".into(), Tag::Int(remaining)),
            ]);
            out.entry(p)
                .or_default()
                .insert(BURNER.into(), Tag::Compound(tag));
        }
        let d = &w.dims[dim];
        for (p, t) in d.basin_fluids.iter().filter(|(p, _)| mine(p)) {
            let tag = Compound::from([
                ("Input".into(), Tag::List(t.input.map(fluid_tag).to_vec())),
                ("Output".into(), Tag::List(t.output.map(fluid_tag).to_vec())),
            ]);
            out.entry(*p)
                .or_default()
                .insert(BASIN.into(), Tag::Compound(tag));
        }
        for (p, i) in d.crafter_inputs.iter().filter(|(p, _)| mine(p)) {
            let tag = Compound::from([
                ("Controller".into(), Tag::Byte(i8::from(i.controller))),
                (
                    "Data".into(),
                    Tag::IntArray(i.data.iter().flatten().copied().collect()),
                ),
            ]);
            out.entry(*p)
                .or_default()
                .insert(INPUT.into(), Tag::Compound(tag));
        }
        for p in d.crafter_covers.iter().filter(|p| mine(p)) {
            out.entry(*p)
                .or_default()
                .insert(COVER.into(), Tag::Byte(1));
        }
    });
}

/// The `read`s of the fields [`save_entities`] writes.
pub fn load_entity(dim: usize, pos: [i32; 3], block: Block, tag: &Compound) {
    let compound = |k: &str| match tag.get(&k.into()) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    };
    with_world(|w| match block {
        Block::Deployer => {
            if let Some(d) = compound(DEPLOYER).and_then(deployer_of) {
                w.load_deployers(dim, std::iter::once((pos, d)));
            }
        }
        Block::CrushingWheelController => {
            let Some(c) = compound(CRUSHER) else { return };
            let slots = match c.get(&"Items".into()) {
                Some(Tag::List(l)) => l
                    .iter()
                    .filter_map(|t| {
                        let Tag::Compound(s) = t else { return None };
                        let Some(Tag::String(id)) = s.get(&"id".into()) else {
                            return None;
                        };
                        Some((id.to_string_lossy(), int(s, "count").clamp(1, 255) as u8))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let remaining = match c.get(&"Remaining".into()) {
                Some(Tag::Float(b)) => f32::from_bits(*b),
                _ => 0.0,
            };
            let crusher = crusher::Crusher {
                entity: None,
                slots,
                remaining,
                applied: matches!(c.get(&"Applied".into()), Some(Tag::Byte(b)) if *b != 0),
            };
            w.load_crushers(dim, std::iter::once((pos, crusher)));
        }
        Block::BlazeBurner => {
            let Some(c) = compound(BURNER) else { return };
            let (fuel, remaining) = (int(c, "Fuel"), int(c, "Remaining"));
            let b = if fuel == CREATIVE_BURNER {
                burner::Burner {
                    creative: Some(heat_of(remaining)),
                    ..burner::Burner::default()
                }
            } else {
                burner::Burner {
                    fuel: burner::Fuel::from_index(fuel),
                    remaining,
                    creative: None,
                }
            };
            w.load_burners(dim, std::iter::once((pos, b)));
        }
        Block::Basin => {
            let tanks = compound(BASIN).map(|c| {
                let pair = |k: &str| match c.get(&k.into()) {
                    Some(Tag::List(l)) => [0, 1].map(|i| l.get(i).and_then(fluid_of)),
                    _ => [None; 2],
                };
                basin::Tanks {
                    input: pair("Input"),
                    output: pair("Output"),
                }
            });
            match tanks {
                Some(t) => w.dims[dim].basin_fluids.insert(pos, t),
                None => w.dims[dim].basin_fluids.remove(&pos),
            };
        }
        Block::MechanicalCrafter => {
            let d = &mut w.dims[dim];
            match compound(INPUT) {
                Some(c) => {
                    let data = match c.get(&"Data".into()) {
                        Some(Tag::IntArray(v)) => v
                            .as_chunks::<3>()
                            .0
                            .iter()
                            .map(|o| [o[0], o[1], o[2]])
                            .collect(),
                        _ => Vec::new(),
                    };
                    let controller =
                        matches!(c.get(&"Controller".into()), Some(Tag::Byte(b)) if *b != 0);
                    d.crafter_inputs
                        .insert(pos, crafter_input::Input { controller, data });
                }
                None => {
                    d.crafter_inputs.remove(&pos);
                }
            }
            if matches!(tag.get(&COVER.into()), Some(Tag::Byte(b)) if *b != 0) {
                d.crafter_covers.insert(pos);
            } else {
                d.crafter_covers.remove(&pos);
            }
        }
        _ => {}
    });
}

/// One deployer as [`deployers_to_nbt`] writes it, without its position.
fn deployer_to_tag(d: &deployer::Deployer) -> Compound {
    let Tag::List(mut list) = deployers_to_nbt(&[([0; 3], d.clone())]) else {
        return Compound::new();
    };
    let Some(Tag::Compound(mut c)) = list.pop() else {
        return Compound::new();
    };
    if let Some(Tag::IntArray(mut v)) = c.remove(&"pos".into()) {
        v.drain(..3);
        c.insert("Values".into(), Tag::IntArray(v));
    }
    c
}

fn deployer_of(c: &Compound) -> Option<deployer::Deployer> {
    let mut c = c.clone();
    if let Some(Tag::IntArray(mut v)) = c.remove(&"Values".into()) {
        v.splice(..0, [0; 3]);
        c.insert("pos".into(), Tag::IntArray(v));
    }
    let data = Compound::from([("deployers".into(), Tag::List(vec![Tag::Compound(c)]))]);
    deployers_from_nbt(&data).pop().map(|(_, d)| d)
}

/// `DeployerBlockEntity.write`: `State`, `Timer`, `Reach`, the held stack and `Mode`, per deployer.
pub(crate) fn deployers_to_nbt(list: &[([i32; 3], deployer::Deployer)]) -> Tag {
    Tag::List(
        list.iter()
            .map(|(p, d)| {
                let mut b = Compound::new();
                let (item, n) = d.held.clone().unwrap_or_default();
                b.insert(
                    "pos".into(),
                    Tag::IntArray(vec![
                        p[0],
                        p[1],
                        p[2],
                        i32::from(d.state.index()),
                        d.timer,
                        d.reach.to_bits() as i32,
                        i32::from(n),
                        i32::from(d.mode.index()),
                        i32::from(d.locked),
                        d.held_damage as i32,
                    ]),
                );
                b.insert("HeldItem".into(), Tag::String(item.as_str().into()));
                if !d.held_components.is_empty() {
                    b.insert(
                        "HeldComponents".into(),
                        Tag::ByteArray(d.held_components.clone()),
                    );
                }
                Tag::Compound(b)
            })
            .collect(),
    )
}

pub(crate) fn deployers_from_nbt(data: &Compound) -> Vec<([i32; 3], deployer::Deployer)> {
    let Some(Tag::List(list)) = data.get(&"deployers".into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let Tag::Compound(b) = b else { return None };
            let Some(Tag::IntArray(v)) = b.get(&"pos".into()) else {
                return None;
            };
            let item = match b.get(&"HeldItem".into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => String::new(),
            };
            let n = (*v.get(6)?).clamp(0, 64) as u8;
            Some((
                [*v.first()?, *v.get(1)?, *v.get(2)?],
                deployer::Deployer {
                    state: deployer::State::from_index(*v.get(3)? as u8),
                    timer: *v.get(4)?,
                    reach: f32::from_bits(*v.get(5)? as u32),
                    held: (!item.is_empty() && n > 0).then_some((item, n)),
                    // Saved before modes: using.
                    mode: deployer::Mode::from_index(v.get(7).copied().unwrap_or(0) as u8),
                    breaking: None,
                    locked: v.get(8).is_some_and(|l| *l != 0),
                    held_damage: v.get(9).map_or(0, |d| (*d).max(0) as u32),
                    held_components: match b.get(&"HeldComponents".into()) {
                        Some(Tag::ByteArray(c)) => c.clone(),
                        _ => Vec::new(),
                    },
                },
            ))
        })
        .collect()
}

/// The `crushers` an older version saved with the column: each controller's position, time
/// left and whether its result was applied, and its slots' ids and counts.
fn crushers_from_nbt(data: &Compound) -> Vec<([i32; 3], crusher::Crusher)> {
    let Some(Tag::List(list)) = data.get(&"crushers".into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let Tag::Compound(b) = b else { return None };
            let Some(Tag::IntArray(pos)) = b.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::List(ids)) = b.get(&"ids".into()) else {
                return None;
            };
            let Some(Tag::IntArray(counts)) = b.get(&"counts".into()) else {
                return None;
            };
            let slots = ids
                .iter()
                .zip(counts)
                .filter_map(|(id, n)| match id {
                    Tag::String(id) => Some((id.to_string_lossy(), (*n).clamp(1, 255) as u8)),
                    _ => None,
                })
                .collect();
            let c = crusher::Crusher {
                entity: None,
                slots,
                remaining: f32::from_bits(*pos.get(3)? as u32),
                applied: *pos.get(4)? != 0,
            };
            Some(([*pos.first()?, *pos.get(1)?, *pos.get(2)?], c))
        })
        .collect()
}

/// A saved burner's fuel type when it is creative: its heat follows.
pub(super) const CREATIVE_BURNER: i32 = 3;

pub(super) fn heat_of(i: i32) -> create_engine::fan::Heat {
    use create_engine::fan::Heat;
    match i {
        2 => Heat::Fading,
        3 => Heat::Kindled,
        4 => Heat::Seething,
        _ => Heat::Smouldering,
    }
}

fn burners_from_nbt(data: &Compound) -> Vec<([i32; 3], burner::Burner)> {
    let Some(Tag::IntArray(v)) = data.get(&"burners".into()) else {
        return Vec::new();
    };
    v.as_chunks::<5>()
        .0
        .iter()
        .map(|c| {
            let b = if c[3] == CREATIVE_BURNER {
                burner::Burner {
                    creative: Some(heat_of(c[4])),
                    ..burner::Burner::default()
                }
            } else {
                burner::Burner {
                    fuel: burner::Fuel::from_index(c[3]),
                    remaining: c[4],
                    creative: None,
                }
            };
            ([c[0], c[1], c[2]], b)
        })
        .collect()
}

fn basins_from_nbt(data: &Compound) -> Vec<([i32; 3], basin::Tanks)> {
    let Some(Tag::IntArray(v)) = data.get(&"basins".into()) else {
        return Vec::new();
    };
    let keys = crate::glue::fluids::fluid_keys(data, "basins");
    v.as_chunks::<11>()
        .0
        .iter()
        .map(|c| {
            let tank = |i: usize| {
                tank::Codes::fluid(&keys, c[3 + 2 * i])
                    .map(|f| (f, c[4 + 2 * i].clamp(0, 1000) as u32))
            };
            (
                [c[0], c[1], c[2]],
                basin::Tanks {
                    input: [tank(0), tank(1)],
                    output: [tank(2), tank(3)],
                },
            )
        })
        .collect()
}

/// Crafters joined with the wrench: x, y, z, controller, the count, then the offsets.
fn inputs_from_nbt(data: &Compound) -> Vec<([i32; 3], crafter_input::Input)> {
    let mut inputs = Vec::new();
    let Some(Tag::IntArray(v)) = data.get(&"crafter_inputs".into()) else {
        return inputs;
    };
    let mut rest = &v[..];
    while let [x, y, z, controller, n, more @ ..] = rest {
        let n = (*n).clamp(0, 4096) as usize;
        let Some(offsets) = more.get(..n * 3) else {
            break;
        };
        let data = offsets
            .as_chunks::<3>()
            .0
            .iter()
            .map(|o| [o[0], o[1], o[2]])
            .collect();
        inputs.push((
            [*x, *y, *z],
            crafter_input::Input {
                controller: *controller != 0,
                data,
            },
        ));
        rest = &more[n * 3..];
    }
    inputs
}

/// Crafters with a slot cover: x, y, z each.
fn covers_from_nbt(data: &Compound) -> Vec<[i32; 3]> {
    match data.get(&"crafter_covers".into()) {
        Some(Tag::IntArray(v)) => v
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2]])
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployers_and_crafter_inputs_read_back_as_saved() {
        let d = deployer::Deployer {
            held: Some(("minecraft:stone".into(), 3)),
            timer: 120,
            locked: true,
            held_damage: 7,
            held_components: vec![10, 0],
            ..Default::default()
        };
        let mut data = Compound::new();
        data.insert(
            "deployers".into(),
            deployers_to_nbt(&[([1, 2, 3], d.clone())]),
        );
        let back = deployers_from_nbt(&data);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].0, [1, 2, 3]);
        assert_eq!(back[0].1.held, d.held);
        assert_eq!(back[0].1.timer, 120);
        assert!(back[0].1.locked);
        assert_eq!(back[0].1.held_damage, 7);
        assert_eq!(back[0].1.held_components, d.held_components);

        // Two crafters: one alone with no offsets, one controller with two.
        data.insert(
            "crafter_inputs".into(),
            Tag::IntArray(vec![0, 0, 0, 0, 0, 5, 6, 7, 1, 2, 1, 0, 0, 0, 1, 0]),
        );
        let inputs = inputs_from_nbt(&data);
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[1].0, [5, 6, 7]);
        assert!(inputs[1].1.controller);
        assert_eq!(inputs[1].1.data, vec![[1, 0, 0], [0, 1, 0]]);
    }
}
