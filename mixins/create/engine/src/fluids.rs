//! Create's fluid pipes and mechanical pump (`FluidPipeBlock`, `PumpBlockEntity`,
//! `FluidPropagator`, `FluidNetwork`, `OpenEndedPipe`): pipes connect to what can hold fluid; a
//! turning pump pulls through the pipes behind it and pushes through those in front, as far as
//! the pump range, from a tank or an open end into a tank or an open end.

use crate::kinetics::Dir;
use crate::tank::Fluid;

/// `mechanicalPumpRange`.
pub const PUMP_RANGE: i32 = 16;

/// What a block is to the pipes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    /// A fluid pipe, open toward its connected sides (`Dir::bit`).
    Pipe(u8),
    /// A pump, its facing.
    Pump(Dir),
    /// A fluid handler (a tank).
    Tank,
    /// Open space a pipe can end at (`isOpenEnd`: replaceable, a fluid, waterloggable).
    Open,
    /// Anything else.
    Solid,
}

/// `FluidPipeBlock.canConnectTo`: toward a pipe, a tank, a pump along its axis.
pub fn connects(neighbour: Cell, toward: Dir) -> bool {
    match neighbour {
        Cell::Pipe(_) | Cell::Tank => true,
        Cell::Pump(f) => f.axis() == toward.axis(),
        _ => false,
    }
}

/// `FluidPipeBlock.updateBlockState` (no brackets): the sides it connects, at least a straight
/// pipe along `preferred` when nothing is around and it wasn't straight before.
pub fn pipe_sides(previous: u8, neighbours: impl Fn(Dir) -> Cell, preferred: Dir) -> u8 {
    let mut sides = 0u8;
    for d in Dir::ALL {
        if connects(neighbours(d), d) {
            sides |= d.bit();
        }
    }
    match sides.count_ones() {
        0 => {
            if previous.count_ones() == 2 {
                previous
            } else {
                preferred.bit() | preferred.opposite().bit()
            }
        }
        1 => {
            let d = Dir::ALL.into_iter().find(|d| sides & d.bit() != 0).unwrap();
            sides | d.opposite().bit()
        }
        _ => sides,
    }
}

/// An end of a pump's pipes: the block fluid comes from or goes to, and the pressure there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct End {
    pub pos: [i32; 3],
    pub tank: bool,
    pub pressure: f32,
}

fn step(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

/// `PumpBlockEntity.distributePressureTo`: the ends reached from the pump's `side` through pipes
/// (breadth first, up to the pump range), the pressure (the pump's speed) shared among the ends
/// at each distance (`parallelBranches`).
pub fn reach(pump: [i32; 3], side: Dir, speed: f32, cell: impl Fn([i32; 3]) -> Cell) -> Vec<End> {
    let mut ends: Vec<(End, i32)> = Vec::new();
    let first = step(pump, side);
    match cell(first) {
        Cell::Tank => ends.push((
            End {
                pos: first,
                tank: true,
                pressure: 0.0,
            },
            0,
        )),
        Cell::Pipe(_) => {
            let mut frontier = std::collections::VecDeque::from([(1, first)]);
            let mut visited = std::collections::HashSet::new();
            while let Some((distance, at)) = frontier.pop_front() {
                if !visited.insert(at) {
                    continue;
                }
                let Cell::Pipe(sides) = cell(at) else {
                    continue;
                };
                for d in Dir::ALL {
                    if sides & d.bit() == 0 {
                        continue;
                    }
                    let next = step(at, d);
                    if next == pump {
                        continue;
                    }
                    match cell(next) {
                        Cell::Tank => ends.push((
                            End {
                                pos: next,
                                tank: true,
                                pressure: 0.0,
                            },
                            distance,
                        )),
                        Cell::Open => ends.push((
                            End {
                                pos: next,
                                tank: false,
                                pressure: 0.0,
                            },
                            distance,
                        )),
                        Cell::Pipe(other) if other & d.opposite().bit() != 0 => {
                            if distance + 1 >= PUMP_RANGE {
                                continue;
                            }
                            if !visited.contains(&next) {
                                frontier.push_back((distance + 1, next));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    let mut out = Vec::new();
    for (end, distance) in &ends {
        let same = ends.iter().filter(|(_, d)| d == distance).count();
        let branches = same.max(1) as f32;
        out.push(End {
            pressure: speed.abs() / branches,
            ..*end
        });
    }
    out
}

/// `FluidNetwork.transferSpeed`: half the pressure where the fluid enters, at least 1 mB a tick.
pub fn transfer_speed(pressure: f32) -> u32 {
    ((pressure / 2.0) as u32).max(1)
}

/// What an open end holds or lets out (`OpenEndedPipe`): the fluid a source block gives (a
/// bucket's worth), placed back as a source once a bucket's worth arrived.
pub fn open_fluid(water: bool, lava: bool) -> Option<Fluid> {
    if water {
        Some(Fluid::Water)
    } else if lava {
        Some(Fluid::Lava)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipe_joins_its_neighbours_and_keeps_an_open_end() {
        let none = |_: Dir| Cell::Solid;
        // Alone: straight along the preferred direction.
        assert_eq!(
            pipe_sides(0, none, Dir::North),
            Dir::North.bit() | Dir::South.bit()
        );
        // Next to a tank on the west: west and, opposite, east (an open end).
        let tank = |d: Dir| {
            if d == Dir::West {
                Cell::Tank
            } else {
                Cell::Solid
            }
        };
        assert_eq!(
            pipe_sides(0, tank, Dir::Up),
            Dir::West.bit() | Dir::East.bit()
        );
        // A pump beside it but across its axis: not joined.
        let pump = |d: Dir| {
            if d == Dir::Up {
                Cell::Pump(Dir::East)
            } else {
                Cell::Solid
            }
        };
        assert_eq!(
            pipe_sides(0, pump, Dir::North),
            Dir::North.bit() | Dir::South.bit()
        );
    }

    #[test]
    fn a_pump_reaches_the_ends_of_its_pipes() {
        // Pump at x 0 facing east; pipes x 1..=3 along x; a tank at x 4, open space at x -2.
        let cell = |p: [i32; 3]| match p {
            [0, 0, 0] => Cell::Pump(Dir::East),
            [1..=3, 0, 0] | [-1, 0, 0] => Cell::Pipe(Dir::East.bit() | Dir::West.bit()),
            [4, 0, 0] => Cell::Tank,
            [-2, 0, 0] => Cell::Open,
            _ => Cell::Solid,
        };
        let front = reach([0, 0, 0], Dir::East, 16.0, cell);
        assert_eq!(
            front,
            vec![End {
                pos: [4, 0, 0],
                tank: true,
                pressure: 16.0
            }]
        );
        let back = reach([0, 0, 0], Dir::West, 16.0, cell);
        assert_eq!(
            back,
            vec![End {
                pos: [-2, 0, 0],
                tank: false,
                pressure: 16.0
            }]
        );
        assert_eq!(transfer_speed(16.0), 8);
        // Two ends at the same distance share the pressure.
        let fork = |p: [i32; 3]| match p {
            [1, 0, 0] => Cell::Pipe(Dir::West.bit() | Dir::North.bit() | Dir::South.bit()),
            [1, 0, -1] | [1, 0, 1] => Cell::Tank,
            [0, 0, 0] => Cell::Pump(Dir::East),
            _ => Cell::Solid,
        };
        let ends = reach([0, 0, 0], Dir::East, 16.0, fork);
        assert_eq!(ends.len(), 2);
        assert!(ends.iter().all(|e| e.pressure == 8.0));
    }
}
