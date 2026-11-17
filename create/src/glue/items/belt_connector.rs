//! `BeltConnectorItem.useOn`: sneaking forgets the first shaft; a click on a shaft remembers
//! it, a click on a second shaft that connects lays the belt (one belt connector used up outside
//! creative) and forgets it. Refusals say nothing, as in Create (its client draws the red
//! particles of `BeltConnectorHandler`). Remembering a shaft, or laying a belt with connectors
//! left, starts the item's 5-tick cooldown; a belt laid gives the `belt` advancement.

use super::super::server::ServerLevel;
use super::super::states;
use super::super::world::with_world;
use super::{creative, custom, held, set_custom, set_held, sneaking};
use create_engine::Block;
use create_engine::belt_connector::{self as bc, FIRST_SHAFT_KEY};
use create_engine::kinetics::Axis;
use mod_api::{
    BlockState, ClientHost, HostBlockStates, HostSide, InteractionResult, ItemPart, ItemStack,
    ServerHost, SoundSource, Tag, UseOnEvent,
};

pub(super) struct Connector;

#[mod_api::horizon_part]
impl ItemPart for Connector {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => use_on(host, event),
            HostSide::Client(host) => predict(host, event),
        }
    }
}

/// `if (world.isClientSide) return validAxis ? SUCCESS : FAIL` (after the sneaking forget).
fn predict(host: &mut dyn ClientHost, event: &UseOnEvent<'_>) -> Option<InteractionResult> {
    let player = host.player(event.player)?;
    if sneaking(&*host, player) {
        return Some(InteractionResult::Success);
    }
    let level = host.level(host.dimension())?;
    let shaft = level
        .block_state(&*host, event.pos)
        .and_then(states::id)
        .and_then(|id| {
            super::super::client_table(
                &*host,
                |t| matches!(t.get(id), Some((Block::Shaft, i)) if i.axis.is_some()),
            )
        })
        .unwrap_or(false);
    Some(if shaft {
        InteractionResult::Success
    } else {
        InteractionResult::Fail
    })
}

/// The world as the belt connector asks it, on the server.
struct Ground<'l, 'a> {
    lv: &'l ServerLevel<'a>,
}

impl bc::Ground for Ground<'_, '_> {
    fn loaded(&self, pos: [i32; 3]) -> bool {
        self.lv.is_loaded(pos)
    }

    fn shaft(&self, pos: [i32; 3]) -> Option<Axis> {
        let state = self.lv.block(pos);
        with_world(|w| match w.table.get(state) {
            Some((Block::Shaft, info)) => info.axis,
            _ => None,
        })
        .flatten()
    }

    fn speed(&self, pos: [i32; 3]) -> f32 {
        let dim = self.lv.dim;
        with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0)
    }

    fn replaceable(&self, pos: [i32; 3]) -> bool {
        self.lv
            .block_state(pos)
            .and_then(|s| self.lv.host.block_states().replaceable(s))
            .unwrap_or(false)
    }
}

/// The belt connector's first shaft (`BELT_FIRST_SHAFT`, a `BlockPos`: three ints).
fn first_shaft(stack: &ItemStack) -> Option<[i32; 3]> {
    match custom(stack).get(&FIRST_SHAFT_KEY.into()) {
        Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
        _ => None,
    }
}

fn use_on(host: &mut dyn ServerHost, event: &UseOnEvent<'_>) -> Option<InteractionResult> {
    let player = host.player(event.player)?;
    let dimension = player.dimension(&*host)?.to_string();
    let back = sneaking(&*host, player);
    let mut lv = ServerLevel::new(host, &dimension)?;
    let pos = event.pos;
    let first = first_shaft(event.stack);
    let click = bc::click(&Ground { lv: &lv }, back, first, pos, true);
    let hand = event.hand;
    let set_first = |lv: &mut ServerLevel<'_>, p: Option<[i32; 3]>| {
        if let Some(mut s) = held(&*lv.host, player, hand) {
            set_custom(
                &mut s,
                &[(FIRST_SHAFT_KEY, p.map(|p| Tag::IntArray(p.to_vec())))],
            );
            set_held(lv.host, player, hand, Some(s));
        }
    };
    let answer = match click {
        bc::Click::Forget => {
            set_first(&mut lv, None);
            InteractionResult::Success
        }
        bc::Click::Fail { forget } => {
            if forget {
                set_first(&mut lv, None);
            }
            InteractionResult::Fail
        }
        bc::Click::Remember => {
            set_first(&mut lv, Some(pos));
            player.set_item_cooldown(lv.host, "create:belt_connector", 5);
            InteractionResult::Success
        }
        bc::Click::Refused => InteractionResult::Success,
        bc::Click::Connect(start) => {
            create_belts(&mut lv, start, pos);
            super::super::advancements::award(lv.host, player, "belt");
            if !creative(&*lv.host, player) {
                super::shrink_held(lv.host, player, hand, 1);
            }
            set_first(&mut lv, None);
            if held(&*lv.host, player, hand).is_some() {
                player.set_item_cooldown(lv.host, "create:belt_connector", 5);
            }
            InteractionResult::Success
        }
    };
    Some(answer)
}

/// `createBelts`: the wool sound, then each block of the chain becomes its belt segment
/// (`switchToBlockState`, waterlogged in a water source; what can't be replaced, the shafts, is
/// broken without drops first); an unbreakable block on the way stops it and takes every belt
/// block laid away again.
pub(crate) fn create_belts(lv: &mut ServerLevel<'_>, start: [i32; 3], end: [i32; 3]) {
    let at = bc::sound_at(start, end).map(|v| f64::from(v) + 0.5);
    lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.wool.place",
        at,
        SoundSource::Blocks,
        0.5,
        1.0,
    );
    let layout = {
        let ground = Ground { lv };
        let Some(axis) = bc::Ground::shaft(&ground, start) else {
            return;
        };
        bc::layout(start, end, axis, |p| bc::Ground::shaft(&ground, p))
    };
    let Some(belt) = lv.host.block_state_default(states::key(Block::Belt)) else {
        return;
    };
    // `getFluidState().getType() == Fluids.WATER`: a water source.
    let water = |lv: &ServerLevel<'_>, s: BlockState| {
        lv.host
            .state_fluid_state(s)
            .is_some_and(|f| f.is_source() && f.fluid.id(&*lv.host) == "minecraft:water")
    };
    let mut failed = false;
    for (pos, seg) in &layout {
        let pos = *pos;
        let Some(state) = lv.block_state(pos) else {
            failed = true;
            break;
        };
        let facts = lv.host.block_states();
        if facts.destroy_speed(state).is_some_and(|h| h < 0.0) {
            failed = true;
            break;
        }
        let replaceable = facts.replaceable(state).unwrap_or(false);
        let waterlogged = water(lv, state);
        if !replaceable {
            // `destroyBlock(pos, false)`: what stays is its fluid.
            lv.destroy_block(pos, false);
        }
        let facts = lv.host.block_states();
        let mut s = belt;
        for (k, v) in [
            ("facing", super::super::kinetics::dir_name(seg.facing)),
            ("slope", bc::slope_name(seg.slope)),
            ("part", bc::part_name(seg.part)),
            ("waterlogged", if waterlogged { "true" } else { "false" }),
        ] {
            s = s.with(&facts, k, v).unwrap_or(s);
        }
        if lv.block_state(pos) != Some(s) {
            super::wrench::switch_state(lv, pos, s);
        }
    }
    if !failed {
        return;
    }
    for (pos, _) in layout {
        let id = lv.block(pos);
        let is_belt = with_world(|w| w.table.get(id).is_some_and(|(b, _)| b == Block::Belt));
        if is_belt == Some(true) {
            lv.destroy_block(pos, false);
        }
    }
}
