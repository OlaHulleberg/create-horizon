//! The pieces drawn on pipes' sides (`PipeAttachmentModel`): connectors, rims and drains as
//! `create_engine::fluids::rim` gives them, and a junction's casing. The server works them out
//! when a block beside a pipe changes and keeps them in the pipe's block entity data (`RIMS`);
//! its clients draw them as parts.

use super::super::server::ServerLevel;
use super::super::states;
use super::info_at;
use super::pipes::pipe_sides_at;
use create_engine::Block;
use create_engine::fluids::{self, Piped, Rim, RimPiece};
use create_engine::kinetics::Dir;
use create_engine::steam::Face;
use mod_api::{BlockEntitySubmitEvent, Compound, Tag};
use std::cell::RefCell;

/// The key of a pipe's pieces in its block entity data: an attachment (`Rim` ordinal) for each
/// side in `Dir::ALL` order, then 1 when its junction casing is drawn.
pub const RIMS: &str = "Rims";

thread_local! {
    /// Blocks whose pieces are worked out again, by level.
    static CHECKS: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

/// A block changed at `pos`: it and the blocks beside it look again.
pub(super) fn changed(dim: usize, pos: [i32; 3]) {
    CHECKS.with_borrow_mut(|c| {
        c.push((dim, pos));
        c.extend(Dir::ALL.map(|d| (dim, super::offset(pos, d))));
    });
}

pub(super) fn reset() {
    CHECKS.with_borrow_mut(Vec::clear);
}

/// What the block at `p` is to a pipe's pieces.
fn piped(lv: &ServerLevel<'_>, p: [i32; 3]) -> Piped {
    let other = |fluid, hose| Piped::Other { fluid, hose };
    let Some((block, info)) = info_at(lv, p) else {
        return other(false, false);
    };
    let axis = |a: Option<create_engine::kinetics::Axis>, f: fn(_) -> Piped| {
        a.map_or(other(false, false), f)
    };
    match block {
        Block::FluidPipe => Piped::Pipe(pipe_sides_at(lv, p)),
        Block::EncasedFluidPipe => Piped::Encased(pipe_sides_at(lv, p)),
        Block::GlassFluidPipe => axis(info.axis, Piped::Glass),
        Block::SmartFluidPipe => axis(
            info.facing
                .map(|f| fluids::smart_pipe_axis(info.face == Some(Face::Wall), f)),
            Piped::Smart,
        ),
        Block::FluidValve => axis(
            info.facing
                .map(|f| fluids::valve_pipe_axis(f, info.along_first)),
            Piped::Valve,
        ),
        Block::MechanicalPump => info.facing.map_or(other(false, false), Piped::Pump),
        Block::FluidTank
        | Block::CreativeFluidTank
        | Block::Spout
        | Block::ItemDrain
        | Block::Basin
        | Block::PortableFluidInterface => other(true, false),
        Block::HosePulley => other(true, true),
        _ => other(false, false),
    }
}

/// The blocks that changed near pipes this tick show their pieces again.
pub(super) fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut checks: Vec<[i32; 3]> = CHECKS.with_borrow_mut(|c| {
        let (mine, rest): (Vec<_>, Vec<_>) = c.drain(..).partition(|(d, _)| *d == dim);
        *c = rest;
        mine.into_iter().map(|(_, p)| p).collect()
    });
    if checks.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        if !lv.is_loaded(pos) {
            continue;
        }
        let own = piped(lv, pos);
        if !own.draws_rims() {
            continue;
        }
        let mut v: Vec<i32> = Dir::ALL
            .into_iter()
            .map(|d| {
                let r = fluids::rim(own, d, piped(lv, super::offset(pos, d)));
                Rim::ALL.iter().position(|x| *x == r).unwrap_or(0) as i32
            })
            .collect();
        v.push(i32::from(
            matches!(own, Piped::Pipe(s) if fluids::draws_casing(s)),
        ));
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let data = Compound::from([(RIMS.into(), Tag::IntArray(v))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

/// Draws the pieces a pipe's block entity data shows (`PipeAttachmentModel.addQuads`), kept
/// with the rest of the block entity's drawing as the kinetic renderer decides.
pub(super) fn draw(event: &mut BlockEntitySubmitEvent<'_>) {
    let Some(Tag::IntArray(v)) = event.entity.nbt.get(&RIMS.into()) else {
        return;
    };
    let v = v.clone();
    if v.len() != 7 {
        return;
    }
    let side = |d: Dir| match d {
        Dir::Down => "down",
        Dir::Up => "up",
        Dir::North => "north",
        Dir::South => "south",
        Dir::West => "west",
        Dir::East => "east",
    };
    for (i, d) in Dir::ALL.into_iter().enumerate() {
        let Some(rim) = Rim::ALL.get(v[i].clamp(0, 6) as usize) else {
            continue;
        };
        for piece in rim.pieces() {
            let kind = match piece {
                RimPiece::Connection => "connection",
                RimPiece::RimConnector => "rim_connector",
                RimPiece::Rim => "rim",
                RimPiece::Drain => "drain",
            };
            if let Some(part) = states::partial(&format!("part_pipe_{kind}_{}", side(d))) {
                event.draw.block(part, &[("facing", "up")]);
            }
        }
    }
    if v[6] != 0
        && let Some(part) = states::partial("part_pipe_casing")
    {
        event.draw.block(part, &[("facing", "up")]);
    }
}
