//! The values the server owns: a scroll on a block (`create:scroll`), a board's pick or question
//! and a value box's short click (`create:value_settings`), what a value box's click takes on
//! the server, and the setting sent back (`create:value`).

use super::super::items::{self, Item};
use super::super::net;
use super::super::server::{ServerLevel, update_all};
use super::super::world::with_world;
use create_engine::Block as B;
use create_engine::board;
use create_engine::kinetics::Dir;
use create_engine::world::StateTable;
use mod_api::{
    Direction, Flow, GameMode, Hand, Host, InteractionResult, NeighbourUpdate, PlayerRef,
    RightClickBlockEvent, ServerHost, SoundSource,
};

/// A player scrolled on a block: position and steps, four little-endian `i32`s.
pub(crate) const SCROLL: &str = "create:scroll";

/// Values set by the wheel or a board, and the value a board asked for, from the server to the
/// player: position and value.
pub(crate) const VALUE: &str = "create:value";

/// A pick on a value board (`ValueSettingsPacket`: position, row, column), a board's question
/// for the current setting (the position alone), or a short click on a value box (position and
/// whether a wrench is in hand), from the client.
pub(crate) const VALUE_SETTINGS: &str = "create:value_settings";

/// `[i32]` as the channels carry them.
pub(super) fn encode(words: &[i32]) -> Vec<u8> {
    words.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// A message's little-endian `i32` words (a trailing part of one left out).
fn words(data: &[u8]) -> Vec<i32> {
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|w| i32::from_le_bytes(*w))
        .collect()
}

/// `ValueSettingsInputHandler.canInteract` (not sneaking, a spectator or in adventure) and no
/// clipboard in the main hand (it copies settings instead).
pub(crate) fn may_use_value_box(host: &dyn Host, player: PlayerRef) -> bool {
    player.may_build(host) == Some(true)
        && !items::sneaking(host, player)
        && !items::is(
            items::held(host, player, Hand::Main).as_ref(),
            Item::Clipboard,
        )
}

/// Whether a hit at `local` (in the block) on its face `side` lands on the active value box of
/// the block at `pos` (`ValueSettingsInputHandler.onBlockActivated`: `isActive`,
/// `Sided.isSideActive`, `testHit`): a creative motor's speed, a brass diode's delay, a
/// distributing brass tunnel's mode. `block_at` reads the engine id of a neighbour.
pub(crate) fn value_box_hit(
    table: &StateTable,
    block_at: impl Fn([i32; 3]) -> u32,
    pos: [i32; 3],
    side: Dir,
    local: [f64; 3],
) -> bool {
    value_box_slot(table, block_at, pos, side).is_some_and(|s| board::hits_box(s, local))
}

/// The centre of the active value box on `side` of the block at `pos`, in the block's own
/// coordinates (`ValueBoxTransform.getLocalOffset`), if it has one there.
pub(crate) fn value_box_slot(
    table: &StateTable,
    block_at: impl Fn([i32; 3]) -> u32,
    pos: [i32; 3],
    side: Dir,
) -> Option<[f64; 3]> {
    let (block, info) = table.get(block_at(pos))?;
    match (block, info.axis, info.tunnel) {
        (B::CreativeMotor, ..) => info.facing.and_then(|f| board::motor_slot(f, side)),
        // `BrassDiodeScrollSlot`: not a sided box, any face reaches it.
        (B::PulseRepeater | B::PulseExtender | B::PulseTimer, ..) => {
            Some(create_engine::diode::SLOT)
        }
        // `selectionMode.onlyActiveWhen(this::hasDistributionBehaviour)`.
        (B::BrassTunnel, Some(axis), Some(shape)) => {
            let tunnel = |d: Dir| {
                let o = d.offset();
                let s = block_at([pos[0] + o[0], pos[1], pos[2] + o[2]]);
                table
                    .get(s)
                    .is_some_and(|(b, _)| matches!(b, B::AndesiteTunnel | B::BrassTunnel))
            };
            create_engine::tunnel::distributes(axis, shape, tunnel)
                .then(|| board::tunnel_mode_slot(side))
                .flatten()
        }
        _ => None,
    }
}

/// `ValueSettingsInputHandler.onBlockActivated` on the server: a click on an active value box is
/// taken (`setCanceled`, `SUCCESS`: the block does not use it) whatever the hand holds but a
/// clipboard. The client holds it to open the board; a short click does no more
/// (`ScrollValueBehaviour.onShortInteract` acts for fake players only).
pub(super) fn right_click(host: &mut dyn ServerHost, event: &mut RightClickBlockEvent<'_>) -> Flow {
    let click = event.click;
    let Some(player) = host.player(click.player) else {
        return Flow::Continue;
    };
    let dimension = click.dimension.key().id();
    let Some(lv) = ServerLevel::new(host, dimension) else {
        return Flow::Continue;
    };
    if lv.block(click.pos) == 0 || !may_use_value_box(&*lv.host, player) {
        return Flow::Continue;
    }
    let Some(side) = Dir::from_offset(click.hit.face) else {
        return Flow::Continue;
    };
    let pos = click.pos;
    let local = [0, 1, 2].map(|i| click.hit.point[i] - f64::from(pos[i]));
    let hit = with_world(|w| value_box_hit(&w.table, |p| lv.block(p), pos, side, local));
    if hit != Some(true) {
        return Flow::Continue;
    }
    *event.cancellation_result = InteractionResult::Success;
    Flow::Cancel
}

/// The player who sent a message about the block at `pos`, checked as a client's packet is:
/// in a level Create runs, within reach of the block (`ServerGamePacketListenerImpl`: the
/// interaction range, plus a margin). The player's level id.
fn checked(host: &dyn Host, player: PlayerRef, pos: [i32; 3]) -> Option<String> {
    let dimension = player.dimension(host)?.to_string();
    super::super::world::level_index(&dimension)?;
    let at = player.entity().position(host)?;
    let center = pos.map(|v| f64::from(v) + 0.5);
    let d2: f64 = (0..3).map(|i| (at[i] - center[i]).powi(2)).sum();
    (d2 <= 8.0 * 8.0).then_some(dimension)
}

/// A player scrolled on a block: a creative motor's or speed controller's speed, a chassis'
/// range (with the wrench, `requiresWrench`), a brass tunnel's mode. The value goes back to
/// the player (its value box shows it).
pub(super) fn scroll_from_client(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let [x, y, z, steps] = words(data)[..] else {
        return;
    };
    let pos = [x, y, z];
    let Some(dimension) = checked(&*host, player, pos) else {
        return;
    };
    let Some(lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let dim = lv.dim;
    let steps = steps.clamp(-64, 64);
    let state = lv.block(pos);
    let block = with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten();
    let value = match block {
        Some(B::CreativeMotor | B::RotationSpeedController) => {
            with_world(|w| w.scroll_motor(dim, pos, steps));
            return;
        }
        // `ChassisScrollValueBehaviour`: the whole group of chassis takes the range, 1 to 16.
        Some(B::LinearChassis | B::SecondaryLinearChassis | B::RadialChassis) => {
            let group = crate::glue::contraptions::chassis_group(&lv, pos);
            let now = with_world(|w| w.dims[dim].chassis_range.get(&pos).copied())
                .flatten()
                .unwrap_or(create_engine::contraption::CHASSIS_RANGE);
            let range = (now + steps).clamp(1, 16);
            with_world(|w| w.set_chassis_range(dim, &group, range));
            range
        }
        Some(B::BrassTunnel) => with_world(|w| {
            let b = w.dims[dim].brass.get_mut(&pos)?;
            b.mode = b.mode.scrolled(steps);
            let i = b.mode.index() as i32;
            w.mark_unsaved(dim, pos);
            Some(i)
        })
        .flatten()
        .unwrap_or(0),
        _ => return,
    };
    net::send_to_player(lv.host, player, VALUE, &encode(&[x, y, z, value]));
}

/// `ValueSettingsPacket.applySettings` (`BlockEntityConfigurationPacket.handle`: no spectator or
/// adventure player), checked as a scroll is. The current setting goes back either way, as the
/// client's board starts on it.
pub(super) fn value_settings_from_client(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    data: &[u8],
) {
    let w = words(data);
    // A short click's last word: whether the main hand holds a wrench (a guest's hand is the
    // client's to see).
    let (pos, pick, short) = match w[..] {
        [x, y, z] if data.len() == 12 => ([x, y, z], None, None),
        [x, y, z, wrench] if data.len() == 16 => ([x, y, z], None, Some(wrench != 0)),
        [x, y, z, row, value] if data.len() == 20 => {
            ([x, y, z], Some(board::Settings { row, value }), None)
        }
        _ => return,
    };
    if matches!(
        player.game_mode(&*host),
        Some(GameMode::Adventure | GameMode::Spectator)
    ) {
        return;
    }
    let Some(dimension) = checked(&*host, player, pos) else {
        return;
    };
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let dim = lv.dim;
    let state = lv.block(pos);
    let Some((block, inverted, facing)) =
        with_world(|w| w.table.get(state).map(|(b, i)| (b, i.inverted, i.facing))).flatten()
    else {
        return;
    };
    use create_engine::tunnel::{self, Mode};
    let value = match block {
        B::CreativeMotor | B::RotationSpeedController => with_world(|w| {
            if let Some(pick) = pick {
                w.board_motor(dim, pos, pick);
            }
            w.motor_setting(dim, pos).map(|rpm| rpm as i32)
        }),
        // `ScrollOptionBehaviour` over Create's seven `SelectionMode`s: `RANDOMIZE` and
        // `SYNCHRONIZE` are not ported and leave the mode as it is (as do connected tunnels,
        // which would take it too).
        B::BrassTunnel => with_world(|w| {
            let b = w.dims[dim].brass.get_mut(&pos)?;
            let now = b.mode.index() as i32;
            let picked =
                pick.and_then(|s| board::option_value(s, now, tunnel::OPTIONS.len() as i32));
            if let Some(mode) = picked.and_then(Mode::from_option) {
                b.mode = mode;
                w.mark_unsaved(dim, pos);
            }
            w.dims[dim].brass.get(&pos).map(|b| b.mode.index() as i32)
        }),
        // `BrassDiodeScrollValueBehaviour`: `onShortInteract` toggles the diode (`toggle`: not
        // with a wrench; sneaking and no building were never a short click); `setValueSettings`
        // sets its delay.
        B::PulseRepeater | B::PulseExtender | B::PulseTimer => {
            if short == Some(false) {
                toggle_diode(&mut lv, pos, inverted, facing);
            }
            with_world(|w| {
                if let Some(pick) = pick {
                    w.set_diode_delay(dim, pos, board::diode_value(pick));
                }
                w.dims[dim].diodes.get(&pos).map(|d| d.max)
            })
        }
        _ => return,
    };
    let Some(value) = value.flatten() else {
        return;
    };
    net::send_to_player(
        lv.host,
        player,
        VALUE,
        &encode(&[pos[0], pos[1], pos[2], value]),
    );
}

/// `BrassDiodeBlock.toggle`: `inverted` flips (`setBlock(.., UPDATE_ALL)`, its output with it:
/// `DiodeBlock.updateNeighborsInFront`) and a lever clicks.
fn toggle_diode(lv: &mut ServerLevel<'_>, pos: [i32; 3], inverted: bool, facing: Option<Dir>) {
    let value = if inverted { "false" } else { "true" };
    if lv.set_property(pos, "inverted", value, update_all())
        && let (Some(facing), Some(state)) = (facing, lv.block_state(pos))
    {
        let o = facing.offset();
        let front = [pos[0] - o[0], pos[1] - o[1], pos[2] - o[2]];
        let except = NeighbourUpdate::NeighboursExcept(direction(facing));
        let _ = lv
            .level
            .update_neighbors(&mut *lv.host, front, state, except);
    }
    let center = pos.map(|v| f64::from(v) + 0.5);
    lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.lever.click",
        center,
        SoundSource::Blocks,
        0.3,
        create_engine::diode::toggle_pitch(inverted),
    );
}

/// The API's direction of the engine's.
pub(crate) fn direction(d: Dir) -> Direction {
    match d {
        Dir::Down => Direction::Down,
        Dir::Up => Direction::Up,
        Dir::North => Direction::North,
        Dir::South => Direction::South,
        Dir::West => Direction::West,
        Dir::East => Direction::East,
    }
}
