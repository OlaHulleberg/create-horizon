//! Andesite and brass tunnels on the server: casing their belt when placed, their shape from
//! what lies beside their segment, and brass tunnels sharing a stack out among their sides
//! (`BrassTunnelBlockEntity.tick`).

use super::super::server::{ServerLevel, update_all};
use super::super::world::with_world;
use super::belt::{belt_segment, insert_into};
use super::depot::depot_items;
use super::info_at;
use create_engine::Block;
use create_engine::belt::Slope;
use create_engine::kinetics::{Axis, Dir};
use create_engine::tunnel;
use mod_api::HostBlockStates;

/// A tunnel's axis and shape at `pos`; None for any other block.
pub fn tunnel_state(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Axis, tunnel::Shape)> {
    let (block, info) = info_at(lv, pos)?;
    matches!(block, Block::AndesiteTunnel | Block::BrassTunnel)
        .then_some((info.axis?, info.tunnel?))
}

/// The brass tunnel's axis at `pos`, if a brass tunnel is there.
fn brass_axis(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Axis> {
    let (block, info) = info_at(lv, pos)?;
    (block == Block::BrassTunnel).then_some(info.axis?)
}

/// `BeltTunnelBlockEntity.flaps` not empty: an open side of the tunnel at `pos` (axis `axis`,
/// shape `shape`) that no tunnel continues.
fn has_flaps(lv: &ServerLevel<'_>, pos: [i32; 3], axis: Axis, shape: tunnel::Shape) -> bool {
    tunnel::sides(axis, shape).into_iter().any(|d| {
        let o = d.offset();
        tunnel_state(lv, [pos[0] + o[0], pos[1], pos[2] + o[2]]).is_none()
    })
}

/// `BrassTunnelBlockEntity.getAdjacent`: the brass tunnel of the same axis beside the one at
/// `pos` across its axis (`left`: counterclockwise from the axis' positive way).
fn brass_adjacent(lv: &ServerLevel<'_>, pos: [i32; 3], axis: Axis, left: bool) -> Option<[i32; 3]> {
    let side = match (axis, left) {
        (Axis::X, true) => Dir::North,
        (Axis::X, false) => Dir::South,
        (_, true) => Dir::East,
        (_, false) => Dir::West,
    };
    let o = side.offset();
    let next = [pos[0] + o[0], pos[1], pos[2] + o[2]];
    (brass_axis(lv, next) == Some(axis)).then_some(next)
}

/// `BrassTunnelBlockEntity.isConnected` on either side: a brass tunnel beside it, both with
/// flaps (`determineIfConnected`).
pub fn brass_connected(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    let Some((axis, shape)) = tunnel_state(lv, pos) else {
        return false;
    };
    has_flaps(lv, pos, axis, shape)
        && [true, false].into_iter().any(|left| {
            brass_adjacent(lv, pos, axis, left).is_some_and(|a| {
                tunnel_state(lv, a).is_some_and(|(ax, sh)| has_flaps(lv, a, ax, sh))
            })
        })
}

/// Andesite tunnels: those just placed case their belt (`BeltTunnelItem`); those whose
/// surroundings changed break without a cased, flat belt below (`canSurvive`) or take the shape
/// the belts and depots beside their segment give (`updateShape`, `getTunnelState`).
pub fn tick_tunnels(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let Some((new, kept, fresh)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.tunnels_new),
            std::mem::take(&mut d.tunnel_checks),
            std::mem::take(&mut d.tunnel_fresh),
        )
    }) else {
        return;
    };
    if new.is_empty() && kept.is_empty() && fresh.is_empty() {
        return;
    }
    let belt_below = |lv: &ServerLevel<'_>, p: [i32; 3]| {
        info_at(lv, [p[0], p[1] - 1, p[2]]).and_then(|(_, i)| i.belt)
    };
    let cased = |lv: &ServerLevel<'_>, p: [i32; 3]| {
        let Some(state) = lv.block_state([p[0], p[1] - 1, p[2]]) else {
            return false;
        };
        let facts = lv.host.block_states();
        state.property(&facts, "casing").ok().flatten() == Some("true")
    };
    for pos in new {
        if tunnel_state(lv, pos).is_none() {
            continue;
        }
        if belt_below(lv, pos).is_some_and(|s| s.slope == Slope::Horizontal) && !cased(lv, pos) {
            lv.set_property([pos[0], pos[1] - 1, pos[2]], "casing", "true", update_all());
        }
    }
    let mut checks: Vec<([i32; 3], bool)> = fresh
        .into_iter()
        .map(|p| (p, true))
        .chain(kept.into_iter().map(|p| (p, false)))
        .collect();
    // A fresh look wins over a kept one for the same tunnel.
    checks.sort_by_key(|c| (c.0, !c.1));
    checks.dedup_by_key(|c| c.0);
    for (pos, at_once) in checks {
        let Some(now) = tunnel_state(lv, pos) else {
            continue;
        };
        let seg = belt_below(lv, pos).filter(|s| s.slope == Slope::Horizontal);
        let Some(seg) = seg.filter(|_| cased(lv, pos)) else {
            lv.destroy_block(pos, true);
            continue;
        };
        let axis = seg.facing.axis();
        // `hasValidOutput` beside the belt segment: a belt across it, or a depot.
        let output = |lv: &ServerLevel<'_>, side: Dir| {
            let o = side.offset();
            let p = [pos[0] + o[0], pos[1] - 1, pos[2] + o[2]];
            match info_at(lv, p) {
                Some((Block::Depot, _)) => true,
                Some((_, i)) => i.belt.is_some_and(|b| b.facing.axis() == side.axis()),
                None => false,
            }
        };
        let left = tunnel::left(axis);
        let (on_left, on_right) = (output(lv, left), output(lv, left.opposite()));
        // `canHaveWindow`: tunnels on both ends.
        let fw = if axis == Axis::X {
            Dir::East
        } else {
            Dir::South
        };
        let window = [fw, fw.opposite()].into_iter().all(|d| {
            let o = d.offset();
            tunnel_state(lv, [pos[0] + o[0], pos[1], pos[2] + o[2]]).is_some()
        });
        let fresh = (axis, tunnel::shape_for(on_left, on_right, window));
        let (axis2, shape2) = if at_once {
            fresh
        } else {
            tunnel::updated(now, fresh)
        };
        if (axis2, shape2) == now {
            continue;
        }
        let Some(state) = lv.block_state(pos) else {
            continue;
        };
        let next = {
            let facts = lv.host.block_states();
            let axis_name = if axis2 == Axis::X { "x" } else { "z" };
            state
                .with(&facts, "axis", axis_name)
                .and_then(|s| s.with(&facts, "shape", shape2.name()))
        };
        if let Ok(next) = next {
            lv.set_state(pos, next, update_all());
        }
    }
}

/// Brass tunnels holding a stack (`BrassTunnelBlockEntity.tick`): two ticks after it arrives
/// (then every `brassTunnelTimer` while some is left), the stack is split among the outputs
/// beside and ahead of the segment: a belt or depot across each open side, the belt ahead.
pub fn tick_brass(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let Some(list) = with_world(|w| w.dims[dim].brass_running.clone()) else {
        return;
    };
    for t in list {
        let Some(mut b) = with_world(|w| w.dims[dim].brass.get(&t).cloned()).flatten() else {
            with_world(|w| w.dims[dim].brass_running.retain(|p| *p != t));
            continue;
        };
        let below = [t[0], t[1] - 1, t[2]];
        if b.progress > 0 {
            b.progress -= 1;
        }
        let belt = belt_segment(lv, below);
        let rpm = belt
            .as_ref()
            .and_then(|(c, ..)| with_world(|w| w.speed(dim, *c)).flatten())
            .unwrap_or(0.0);
        let save = |b: &tunnel::Brass| {
            with_world(|w| {
                let d = &mut w.dims[dim];
                if let Some(x) = d.brass.get_mut(&t) {
                    *x = b.clone();
                }
                if b.stack.is_none() {
                    d.brass_running.retain(|p| *p != t);
                }
            });
        };
        let (Some((_, _, seg)), Some((item, count))) = (belt, b.stack.clone()) else {
            save(&b);
            continue;
        };
        if rpm == 0.0 {
            save(&b);
            continue;
        }
        let movement = create_engine::belt::movement_facing(rpm, seg.facing);
        let Some((axis, shape)) = tunnel_state(lv, t) else {
            continue;
        };
        // `gatherValidOutputs`: this tunnel's sides across first, then ahead (never back), then
        // what lies ahead and beside the brass tunnels next to it across its axis
        // (`addValidOutputsOf`), as targets beside a belt segment.
        let open = tunnel::sides(axis, shape);
        let order = [Dir::South, Dir::West, Dir::North, Dir::East];
        let takes = |p: [i32; 3]| {
            with_world(|w| {
                w.dims[dim].depots.contains_key(&p) || w.dims[dim].belt_of.contains_key(&p)
            }) == Some(true)
        };
        let beside = |at: [i32; 3], d: Dir| {
            let o = d.offset();
            [at[0] + o[0], at[1], at[2] + o[2]]
        };
        let mut outputs: Vec<([i32; 3], Dir)> = order
            .iter()
            .filter(|d| d.axis() != movement.axis())
            .chain(order.iter().filter(|d| **d == movement))
            .copied()
            .filter(|d| open.contains(d) && Some(*d) != b.entered_from)
            .filter(|d| takes(beside(below, *d)))
            .map(|d| (below, d))
            .collect();
        for left in [true, false] {
            let mut at = t;
            while let Some(next) = brass_adjacent(lv, at, axis, left) {
                at = next;
                let Some((next_axis, next_shape)) = tunnel_state(lv, at) else {
                    break;
                };
                let seg = [at[0], at[1] - 1, at[2]];
                let Some((c, _, s)) = belt_segment(lv, seg) else {
                    continue;
                };
                let speed = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
                let moving = create_engine::belt::movement_facing(speed, s.facing);
                for d in order {
                    if d == moving.opposite()
                        || (d == moving && speed == 0.0)
                        || !tunnel::sides(next_axis, next_shape).contains(&d)
                        || !takes(beside(seg, d))
                    {
                        continue;
                    }
                    outputs.push((seg, d));
                }
            }
        }
        if b.progress == -1 {
            if outputs.is_empty() {
                save(&b);
                continue;
            }
            if b.new_item {
                b.new_item = false;
                b.progress = 2;
            } else {
                b.progress = tunnel::BRASS_TIMER;
                save(&b);
                continue;
            }
        }
        if b.progress != 0 {
            save(&b);
            continue;
        }
        // `insertIntoTunnel`: a depot takes a stack while empty, a belt one it has room for.
        let left = tunnel::distribute(
            count,
            outputs.len(),
            b.previous_output,
            b.mode,
            |i, n, simulate| {
                let (at, d) = outputs[i];
                let p = beside(at, d);
                if simulate {
                    let depot = with_world(|w| w.dims[dim].depots.contains_key(&p)) == Some(true);
                    if depot && !depot_items(lv, p).is_empty() {
                        return Some(n);
                    }
                    return Some(0);
                }
                let mut one = create_engine::belt::Item::new(&item, n, 180, false);
                one.progress = b.stack_progress.clone();
                insert_into(lv, p, d, &one)
            },
        );
        b.stack = (left > 0).then(|| (item.clone(), left));
        if b.stack.is_none() {
            b.entered_from = None;
            b.stack_progress = None;
        }
        b.previous_output = (b.previous_output + 1) % outputs.len().max(1);
        b.progress = -1;
        save(&b);
        with_world(|w| w.mark_unsaved(dim, t));
    }
}
