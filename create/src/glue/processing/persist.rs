//! The machines' state saved with each column, under these keys: `deployers`
//! (`DeployerBlockEntity.write`), `crushers` (the controllers' contents), `burners` (fuel and
//! burn time), `basins` (their tanks), `crafter_inputs` (crafters joined with the wrench) and
//! `crafter_covers` (slot covers).

use crate::glue::world::with_world;
use create_engine::{basin, burner, crafter_input, crusher, deployer, tank};
use mod_api::{Compound, Tag};

/// What column data `data` saved for the machines, handed to the engine before the column's
/// blocks are found.
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

/// The machines' part of column `chunk`'s data.
pub fn save(dim: usize, chunk: [i32; 2], data: &mut Compound) {
    let Some(saved) = with_world(|w| {
        let (cx, cz) = (chunk[0], chunk[1]);
        let mine = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
        let d = &w.dims[dim];
        let mut basins: Vec<([i32; 3], basin::Tanks)> = d
            .basin_fluids
            .iter()
            .filter(|(p, _)| mine(p))
            .map(|(p, t)| (*p, *t))
            .collect();
        basins.sort_by_key(|e| e.0);
        let mut covers: Vec<[i32; 3]> = d
            .crafter_covers
            .iter()
            .filter(|p| mine(p))
            .copied()
            .collect();
        covers.sort();
        let mut inputs: Vec<(&[i32; 3], &crafter_input::Input)> =
            d.crafter_inputs.iter().filter(|(p, _)| mine(p)).collect();
        inputs.sort_by_key(|e| *e.0);
        let inputs: Vec<i32> = inputs
            .into_iter()
            .flat_map(|(p, i)| {
                [
                    p[0],
                    p[1],
                    p[2],
                    i32::from(i.controller),
                    i.data.len() as i32,
                ]
                .into_iter()
                .chain(i.data.iter().flatten().copied())
            })
            .collect();
        (
            w.column_deployers(dim, cx, cz),
            w.column_crushers(dim, cx, cz),
            w.column_burners(dim, cx, cz),
            basins,
            covers,
            inputs,
        )
    }) else {
        return;
    };
    let (deployers, crushers, burners, basins, covers, inputs) = saved;
    if !deployers.is_empty() {
        data.insert("deployers".into(), deployers_to_nbt(&deployers));
    }
    if !crushers.is_empty() {
        data.insert("crushers".into(), crushers_to_nbt(&crushers));
    }
    if !burners.is_empty() {
        // x, y, z, fuel type, burn time left for each burner (`fuelLevel`, `burnTimeRemaining`);
        // a creative one (`isCreative`) as fuel type 3 and its heat.
        let v = burners
            .iter()
            .flat_map(|(p, b)| match b.creative {
                Some(h) => [p[0], p[1], p[2], CREATIVE_BURNER, h as i32],
                None => [p[0], p[1], p[2], b.fuel.index(), b.remaining],
            })
            .collect();
        data.insert("burners".into(), Tag::IntArray(v));
    }
    if !basins.is_empty() {
        // x, y, z, then fluid (255: none; a potion by its key beside) and amount for each of
        // the four tanks.
        let mut codes = tank::Codes::default();
        let v = basins
            .into_iter()
            .flat_map(|(p, t)| {
                let mut out = p.to_vec();
                for f in t.input.iter().chain(&t.output) {
                    out.push(codes.code(f.map(|f| f.0)));
                    out.push(f.map_or(0, |f| f.1 as i32));
                }
                out
            })
            .collect();
        data.insert("basins".into(), Tag::IntArray(v));
        crate::glue::fluids::put_fluid_keys(data, "basins", codes);
    }
    if !inputs.is_empty() {
        data.insert("crafter_inputs".into(), Tag::IntArray(inputs));
    }
    if !covers.is_empty() {
        data.insert(
            "crafter_covers".into(),
            Tag::IntArray(covers.into_iter().flatten().collect()),
        );
    }
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

fn crushers_to_nbt(crushers: &[([i32; 3], crusher::Crusher)]) -> Tag {
    Tag::List(
        crushers
            .iter()
            .map(|(p, c)| {
                let mut b = Compound::new();
                b.insert(
                    "pos".into(),
                    Tag::IntArray(vec![
                        p[0],
                        p[1],
                        p[2],
                        c.remaining.to_bits() as i32,
                        i32::from(c.applied),
                    ]),
                );
                b.insert(
                    "ids".into(),
                    Tag::List(
                        c.slots
                            .iter()
                            .map(|s| Tag::String(s.0.as_str().into()))
                            .collect(),
                    ),
                );
                b.insert(
                    "counts".into(),
                    Tag::IntArray(c.slots.iter().map(|s| i32::from(s.1)).collect()),
                );
                Tag::Compound(b)
            })
            .collect(),
    )
}

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
