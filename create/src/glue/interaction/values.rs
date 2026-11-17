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

/// Values set by a board, and the value a board asked for, from the server to the
/// player: position and value.
pub(crate) const VALUE: &str = "create:value";

/// A pick on a value board (`ValueSettingsPacket`: position, row, column), a board's question
/// for the current setting (the position alone), or a short click on a value box (position and
/// whether a wrench is in hand), from the client.
pub(crate) const VALUE_SETTINGS: &str = "create:value_settings";

/// The key in a kinetic block entity's client data counting the looks at its value box.
pub(crate) const LOOKS: &str = "create:value_looks";

thread_local! {
    /// How often players asked for each value box's setting while looking at it.
    static LOOKED: std::cell::RefCell<std::collections::HashMap<[i32; 3], i32>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// How often players looked at the value box at `pos`, if ever.
pub(crate) fn looks(pos: [i32; 3]) -> Option<i32> {
    LOOKED.with_borrow(|m| m.get(&pos).copied())
}

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

/// Read the behaviour's activity from the current client block entity.
pub(crate) fn client_active(host: &dyn mod_api::ClientHost, pos: [i32; 3]) -> bool {
    let block = super::super::client_table(host, |t| {
        t.get(super::board::block_at(host, pos)).map(|(b, _)| b)
    })
    .flatten();
    let data = host
        .level(host.dimension())
        .and_then(|l| host.block_entity(l, pos))
        .and_then(|e| e.data(host));
    match block.map(board::Activity::of) {
        Some(board::Activity::IdleValve) => data
            .as_ref()
            .is_none_or(super::super::sources::handles::value_active),
        Some(board::Activity::UndrivenSteamShaft) => super::super::client_table(host, |t| {
            let Some((_, info)) = t.get(super::board::block_at(host, pos)) else {
                return true;
            };
            let Some(connected) = info
                .face
                .zip(info.facing)
                .map(|(face, facing)| create_engine::steam::connected_direction(face, facing))
            else {
                return true;
            };
            let shaft = create_engine::steam::shaft_pos(pos, connected);
            if t.get(super::board::block_at(host, shaft)).map(|(b, _)| b) != Some(B::PoweredShaft) {
                return true;
            }
            host.level(host.dimension())
                .and_then(|l| host.block_entity(l, shaft))
                .and_then(|e| e.data(host))
                .is_none_or(|d| {
                    let Some(mod_api::Tag::Compound(ports)) = d.get(&"KineticState".into()) else {
                        return true;
                    };
                    let Some(mod_api::Tag::Compound(port)) = ports.get(&"0".into()) else {
                        return true;
                    };
                    !port.contains_key(&"Source".into())
                })
        })
        .unwrap_or(true),
        Some(board::Activity::DistributingTunnel) => super::super::client_table(host, |t| {
            let Some((_, info)) = t.get(super::board::block_at(host, pos)) else {
                return false;
            };
            match (info.axis, info.tunnel) {
                (Some(axis), Some(shape)) => {
                    create_engine::tunnel::distributes(axis, shape, |side| {
                        let o = side.offset();
                        t.get(super::board::block_at(
                            host,
                            [pos[0] + o[0], pos[1], pos[2] + o[2]],
                        ))
                        .is_some_and(|(b, _)| matches!(b, B::AndesiteTunnel | B::BrassTunnel))
                    })
                }
                _ => false,
            }
        })
        .unwrap_or(false),
        Some(board::Activity::BrassFilter) => {
            matches!(block, Some(B::BrassFunnel | B::BrassBeltFunnel))
        }
        _ => true,
    }
}

/// Whether a hit at `local` (in the block) on its face `side` lands on the active value box of
/// the block at `pos` (`ValueSettingsInputHandler.onBlockActivated`: `isActive`,
/// `Sided.isSideActive`, `testHit`): a creative motor's speed, a brass diode's delay, a
/// distributing brass tunnel's mode. `block_at` reads the engine id of a neighbour.
pub(crate) fn value_box_hit(
    table: &StateTable,
    block_at: impl Fn([i32; 3]) -> u32,
    charged: impl Fn([i32; 3]) -> bool,
    pos: [i32; 3],
    side: Dir,
    active: bool,
    local: [f64; 3],
) -> bool {
    let Some((block, info)) = table.get(block_at(pos)) else {
        return false;
    };
    if !active {
        return false;
    }
    // Boxes tested by their own reach.
    if create_engine::dye::is_valve_handle(block) {
        return info
            .facing
            .and_then(|f| board::valve_slot(f, side))
            .is_some_and(|s| board::hits_valve_box(s, local));
    }
    if block == B::MechanicalRoller {
        return info.facing.is_some_and(|f| {
            let (slot, r) = create_engine::actors::roller_slot(f, false);
            let d2: f64 = (0..3).map(|i| (local[i] - slot[i]).powi(2)).sum();
            d2.sqrt() < r
        });
    }
    value_box_slot(table, block_at, charged, pos, side, active)
        .is_some_and(|s| board::hits_box(s, local))
}

/// The centre of the active value box on `side` of the block at `pos`, in the block's own
/// coordinates (`ValueBoxTransform.getLocalOffset`), if it has one there.
pub(crate) fn value_box_slot(
    table: &StateTable,
    block_at: impl Fn([i32; 3]) -> u32,
    charged: impl Fn([i32; 3]) -> bool,
    pos: [i32; 3],
    side: Dir,
    active: bool,
) -> Option<[f64; 3]> {
    if !active {
        return None;
    }
    let (block, info) = table.get(block_at(pos))?;
    if create_engine::dye::is_valve_handle(block) {
        return info.facing.and_then(|f| board::valve_slot(f, side));
    }
    match (block, info.axis, info.tunnel) {
        (B::CreativeMotor, ..) => info.facing.and_then(|f| board::motor_slot(f, side)),
        (B::RotationSpeedController, Some(axis), _) => board::controller_slot(axis, side),
        // `IBearingBlockEntity.getMovementModeSlot`, the windmill's direction and the clock's
        // hands on the same slot.
        (B::MechanicalBearing | B::WindmillBearing | B::ClockworkBearing, ..) => info
            .facing
            .and_then(|f| board::extender_slot(f, None, side)),
        // `MechanicalPistonBlockEntity.getMovementModeSlot`: off its shaft's axis too.
        (B::MechanicalPiston | B::StickyMechanicalPiston, ..) => info.facing.and_then(|f| {
            let shaft = create_engine::parts::directional_axis(f, info.along_first);
            board::extender_slot(f, Some(shaft), side)
        }),
        // `SteamEngineValueBox`: its rotation direction, on the two sides not recessed.
        (B::SteamEngine, ..) => match (info.face, info.facing) {
            (Some(face), Some(facing)) => create_engine::steam::value_box_slot(face, facing, side),
            _ => None,
        },
        // `ChassisBlockEntity.range` (`CenteredSideValueBoxTransform`): the middle of every side,
        // shown and used with the wrench only (`requiresWrench`, `requires_wrench`).
        (B::LinearChassis | B::SecondaryLinearChassis | B::RadialChassis, ..) => {
            Some(board::centred_slot(side))
        }
        // `PulleyBlockEntity.getMovementModeSlot`: on top.
        (B::RopePulley, ..) => (side == Dir::Up).then(|| board::centred_slot(side)),
        // `RollerBlockEntity.RollerValueBox(-3)`: the mode, beside its filter, from any side.
        (B::MechanicalRoller, ..) => info
            .facing
            .map(|f| create_engine::actors::roller_slot(f, false).0),
        // `BrassDiodeScrollSlot`: not a sided box, any face reaches it.
        (B::PulseRepeater | B::PulseExtender | B::PulseTimer, ..) => {
            Some(create_engine::diode::SLOT)
        }
        // `ArmBlockEntity.selectionMode`.
        (B::MechanicalArm, ..) => board::arm_mode_slot(info.ceiling, side),
        // `CartAssemblerValueBoxTransform`: on the sides across the rail, 7 px up, 17.5 out.
        (B::CartAssembler, ..) => {
            let across = if info.rail == Some(create_engine::rail::Shape::NorthSouth) {
                create_engine::kinetics::Axis::X
            } else {
                create_engine::kinetics::Axis::Z
            };
            (side.axis() == across)
                .then(|| board::sided_offset([0.5, 7.0 / 16.0, 17.5 / 16.0], side))
        }
        // `EjectorBlockEntity.maxStackSize`: its top only while the plate is not charged.
        (B::WeightedEjector, ..) => info
            .facing
            .and_then(|f| board::ejector_slot(f, side, charged(pos))),
        (B::BrassTunnel, ..) => board::tunnel_mode_slot(side),
        _ => None,
    }
}

/// `ValueSettingsBehaviour.onlyVisibleWithWrench`: a chassis' range box is seen and used only
/// with the wrench in the main hand (`ValueSettingsInputHandler`, `ValueBox` outlines).
pub(crate) fn requires_wrench(block: B) -> bool {
    matches!(
        block,
        B::LinearChassis | B::SecondaryLinearChassis | B::RadialChassis
    )
}

/// Whether the box of the block in `state` may be used with or without the wrench in hand.
pub(crate) fn wrench_fits(table: &StateTable, state: u32, wrench: bool) -> bool {
    table
        .get(state)
        .is_none_or(|(b, _)| !requires_wrench(b) || wrench)
}

/// The title and option names of a block's `ScrollOptionBehaviour` (an icons board).
pub(crate) fn option_names(block: B) -> Option<(&'static str, &'static [&'static str])> {
    Some(match block {
        B::BrassTunnel => ("Distribution Method", &create_engine::tunnel::OPTIONS[..]),
        // `ArmBlockEntity.selectionMode`.
        B::MechanicalArm => ("Distribution Method", &board::ARM_MODES[..]),
        B::MechanicalBearing => ("Movement Mode", &board::ROTATION_MODES[..]),
        B::MechanicalPiston | B::StickyMechanicalPiston | B::RopePulley => {
            ("Movement Mode", &board::MOVEMENT_MODES[..])
        }
        // `contraptions.windmill.rotation_direction`, the windmill's and the steam engine's.
        B::WindmillBearing | B::SteamEngine => {
            ("Rotation Direction", &board::ROTATION_DIRECTIONS[..])
        }
        B::ClockworkBearing => ("Clock Hand Arrangement", &board::CLOCK_HANDS[..]),
        B::MechanicalRoller => ("Roller Mode", &board::ROLLER_MODES[..]),
        B::CartAssembler => (
            "Cart Movement Mode",
            &create_engine::mounted::MODE_OPTIONS[..],
        ),
        _ => return None,
    })
}

/// The icons of a block's `ScrollOptionBehaviour` options (`INamedIconOptions.getIcon`), in
/// the order of [`option_names`]: each icon's top left in `create:gui/icons` (`AllIcons`).
pub(crate) fn option_icons(block: B) -> &'static [[f32; 2]] {
    /// `AllIcons` column `c` of row `r`.
    const fn at(c: u8, r: u8) -> [f32; 2] {
        [c as f32 * 16.0, r as f32 * 16.0]
    }
    match block {
        B::BrassTunnel => {
            const {
                &[
                    at(4, 3),
                    at(5, 3),
                    at(6, 3),
                    at(7, 3),
                    at(8, 3),
                    at(9, 3),
                    at(10, 3),
                ]
            }
        }
        B::MechanicalArm => const { &[at(4, 4), at(5, 4), at(6, 4)] },
        B::MechanicalBearing => const { &[at(6, 1), at(7, 1), at(8, 1)] },
        B::MechanicalPiston | B::StickyMechanicalPiston | B::RopePulley => {
            const { &[at(9, 1), at(10, 1), at(11, 1)] }
        }
        // `I_REFRESH`, `I_ROTATE_CCW`.
        B::WindmillBearing | B::SteamEngine => const { &[at(3, 1), at(4, 5)] },
        B::ClockworkBearing => const { &[at(5, 5), at(6, 5), at(7, 5)] },
        B::MechanicalRoller => const { &[at(9, 4), at(10, 4), at(11, 4)] },
        B::CartAssembler => const { &[at(12, 1), at(13, 1), at(14, 1)] },
        _ => &[],
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
    let dim = lv.dim;
    let charged_here = with_world(|w| {
        w.dims[dim]
            .ejectors
            .get(&pos)
            .is_none_or(|e| e.state == create_engine::ejector::State::Charged)
    })
    .unwrap_or(true);
    let charged = |_: [i32; 3]| charged_here;
    let wrench = items::is(
        items::held(&*lv.host, player, Hand::Main).as_ref(),
        Item::Wrench,
    );
    let hit = with_world(|w| {
        wrench_fits(&w.table, lv.block(pos), wrench)
            && value_box_hit(
                &w.table,
                |p| lv.block(p),
                charged,
                pos,
                side,
                w.value_box_active(dim, pos),
                local,
            )
    });
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
    let Some(value) = settings_at(&mut lv, player, pos, pick, short) else {
        return;
    };
    if pick.is_some() {
        crate::glue::advancements::speed_controller(&mut lv, pos);
    }
    // A question alone (no pick, no click) is the client looking at the block: its block entity
    // is sent again, so the client draws it anew with the value (`value_text`).
    if pick.is_none() && short.is_none() {
        LOOKED.with_borrow_mut(|m| {
            let n = m.entry(pos).or_insert(0);
            *n = n.wrapping_add(1);
        });
        lv.host.block_entity_sync(pos);
    }
    net::send_to_player(
        lv.host,
        player,
        VALUE,
        &encode(&[pos[0], pos[1], pos[2], value]),
    );
}

/// `ScrollValueBehaviour.getClipboardKey` of a block whose value box `settings_at` serves: the
/// motors' `KineticScrollValueBehaviour` ("Speed"), a `ScrollOptionBehaviour`'s enum's name, the
/// diode's "Timings", any other "Settings".
pub(crate) fn clipboard_key(block: B) -> Option<&'static str> {
    Some(match block {
        B::CreativeMotor | B::RotationSpeedController => "Speed",
        B::MechanicalArm | B::BrassTunnel => "SelectionMode",
        B::MechanicalBearing => "RotationMode",
        B::MechanicalPiston | B::StickyMechanicalPiston | B::RopePulley => "MovementMode",
        B::WindmillBearing | B::SteamEngine => "RotationDirection",
        B::ClockworkBearing => "ClockHands",
        B::MechanicalRoller => "RollingMode",
        B::CartAssembler => "CartMovementMode",
        B::PulseRepeater | B::PulseExtender | B::PulseTimer => "Timings",
        B::WeightedEjector => "Settings",
        b if create_engine::dye::is_valve_handle(b) => "Settings",
        _ => return None,
    })
}

/// `getValueSettings` of a block's value `v` as its board shows it: an option or an ejector's
/// stack size on row 0, a diode's delay on its unit's row, a speed or angle by its sign.
pub(crate) fn board_pick(block: B, v: i32) -> board::Settings {
    if option_names(block).is_some() || block == B::WeightedEjector {
        board::Settings { row: 0, value: v }
    } else if create_engine::world::diode_kind(block).is_some() {
        board::diode_settings(v)
    } else {
        board::kinetic_settings(v)
    }
}

/// The value box of the block at `pos` (`setValueSettings` with `pick`; `onShortInteract` for a
/// `short` click, with whether a wrench is held): its value after, None when the block has no
/// box served here.
pub(crate) fn settings_at(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    pick: Option<board::Settings>,
    short: Option<bool>,
) -> Option<i32> {
    let dim = lv.dim;
    let state = lv.block(pos);
    let (block, inverted, facing) =
        with_world(|w| w.table.get(state).map(|(b, i)| (b, i.inverted, i.facing))).flatten()?;
    let lv = &mut *lv;
    use create_engine::tunnel::{self, Mode};
    let value = match block {
        B::CreativeMotor | B::RotationSpeedController => with_world(|w| {
            if let Some(pick) = pick {
                w.board_motor(dim, pos, pick);
            }
            w.motor_setting(dim, pos).map(|rpm| rpm as i32)
        }),
        // `ArmBlockEntity.selectionMode` (`ScrollOptionBehaviour`).
        B::MechanicalArm => with_world(|w| {
            let arm = w.dims[dim].arms.arms.get_mut(&pos)?;
            let now = arm.selection.index();
            let n = board::ARM_MODES.len() as i32;
            if let Some(v) = pick.and_then(|s| board::option_value(s, now, n)) {
                arm.selection = create_engine::arm::Selection::from_index(v);
                w.mark_unsaved(dim, pos);
            }
            w.dims[dim].arms.arms.get(&pos).map(|a| a.selection.index())
        }),
        // `ChassisScrollValueBehaviour`: the whole group takes the range picked (1 to 16).
        B::LinearChassis | B::SecondaryLinearChassis | B::RadialChassis => {
            let now = with_world(|w| w.dims[dim].chassis_range.get(&pos).copied())
                .flatten()
                .unwrap_or(create_engine::contraption::CHASSIS_RANGE);
            match pick.and_then(|s| board::chassis_value(s, now, 16)) {
                Some(range) => {
                    let group = crate::glue::contraptions::chassis_group(lv, pos);
                    with_world(|w| w.set_chassis_range(dim, &group, range));
                    Some(Some(range))
                }
                None => Some(Some(now)),
            }
        }
        // `EjectorBlockEntity.maxStackSize` (`between(0, 64)`, row 0).
        B::WeightedEjector => with_world(|w| {
            let e = w.dims[dim].ejectors.get_mut(&pos)?;
            if let Some(pick) = pick {
                e.max_stack = pick.value.clamp(0, 64) as u8;
                w.mark_unsaved(dim, pos);
                w.dims[dim].ejecting.retain(|p| *p != pos);
                w.dims[dim].ejecting.push(pos);
            }
            w.dims[dim]
                .ejectors
                .get(&pos)
                .map(|e| i32::from(e.max_stack))
        }),
        // `ScrollOptionBehaviour` over Create's seven `SelectionMode`s; tunnels connected with
        // it take the mode too.
        B::BrassTunnel => {
            let now = with_world(|w| w.dims[dim].brass.get(&pos).map(|b| b.mode.index() as i32))
                .flatten();
            let picked = pick.and_then(|s| {
                board::option_value(s, now.unwrap_or(0), tunnel::OPTIONS.len() as i32)
            });
            if let Some(mode) = picked.and_then(Mode::from_option) {
                crate::glue::logistics::set_brass_mode(lv, pos, mode);
            }
            Some(
                with_world(|w| w.dims[dim].brass.get(&pos).map(|b| b.mode.index() as i32))
                    .flatten(),
            )
        }
        // The contraption blocks' `ScrollOptionBehaviour`s (their movement mode, a windmill's
        // direction, a clock's hands, a roller's mode).
        B::MechanicalBearing
        | B::WindmillBearing
        | B::ClockworkBearing
        | B::MechanicalPiston
        | B::StickyMechanicalPiston
        | B::RopePulley
        | B::MechanicalRoller
        | B::SteamEngine => {
            use crate::glue::contraptions::{scroll_option, set_scroll_option};
            // `SteamEngineBlockEntity.movementDirection.onlyActiveWhen`: not while another
            // source turns its shaft.
            if block == B::SteamEngine && crate::glue::fluids::engine_sourced(lv, pos) {
                return None;
            }
            let count = option_names(block).map_or(1, |(_, o)| o.len() as i32);
            let now = scroll_option(lv, pos, block);
            if let Some(v) = pick.and_then(|s| board::option_value(s, now, count)) {
                set_scroll_option(lv, pos, block, v);
            }
            Some(Some(scroll_option(lv, pos, block)))
        }
        // `TableClothFilteringBehaviour`: a shop's price tag.
        b if crate::glue::stock::shop::is_cloth(b) => Some(
            crate::glue::stock::shop::value_settings(lv, pos, player, pick, short.is_some()),
        ),
        // `FilteringBehaviour`: a short click sets the filter, a pick its amount.
        B::BrassFunnel | B::BrassBeltFunnel | B::SmartChute => Some(
            crate::glue::logistics::amount::value_settings(lv, pos, player, pick, short.is_some()),
        ),
        // `CartAssemblerBlockEntity.movementMode`.
        B::CartAssembler => {
            let count = create_engine::mounted::MODE_OPTIONS.len() as i32;
            let now = crate::glue::trains::assembler_mode(dim, pos);
            if let Some(v) = pick.and_then(|s| board::option_value(s, now, count)) {
                crate::glue::trains::set_assembler_mode(dim, pos, v);
            }
            Some(Some(crate::glue::trains::assembler_mode(dim, pos)))
        }
        // `BrassDiodeScrollValueBehaviour`: `onShortInteract` toggles the diode (`toggle`: not
        // with a wrench; sneaking and no building were never a short click); `setValueSettings`
        // sets its delay.
        B::PulseRepeater | B::PulseExtender | B::PulseTimer => {
            if short == Some(false) {
                toggle_diode(lv, pos, inverted, facing);
            }
            let max = with_world(|w| {
                if let Some(pick) = pick {
                    w.set_diode_delay(dim, pos, board::diode_value(pick));
                }
                w.dims[dim].diodes.get(&pos).map(|d| d.max)
            });
            if pick.is_some() {
                crate::glue::redstone::show_diode(lv, pos);
            }
            max
        }
        // `ValveHandleScrollValueBehaviour`: `onShortInteract` uses the handle as a click on it
        // would (`ValveHandleBlock.clicked`); `setValueSettings` sets its angle, never 0.
        b if create_engine::dye::is_valve_handle(b) => {
            if short.is_some()
                && let Some(state) = lv.block_state(pos)
            {
                let item =
                    items::held(&*lv.host, player, Hand::Main).map(|s| s.item().id(&*lv.host));
                let back = items::sneaking(&*lv.host, player);
                super::super::blocks::use_valve_handle(
                    lv,
                    player,
                    pos,
                    b,
                    state,
                    item.as_deref(),
                    back,
                );
            }
            let entity = lv.host.block_entity(lv.level, pos);
            if let (Some(pick), Some(entity)) = (pick, entity) {
                let value = board::kinetic_value(pick, 180);
                let data = mod_api::Compound::from([(
                    super::super::sources::handles::SCROLL_VALUE.into(),
                    mod_api::Tag::Int(value),
                )]);
                let _ = entity.merge_data(&mut *lv.host, data);
            }
            Some(
                lv.host
                    .block_entity(lv.level, pos)
                    .and_then(|e| e.data(&*lv.host))
                    .map(|d| super::super::sources::handles::valve_angle(&d)),
            )
        }
        _ => return None,
    };
    if pick.is_some() || short.is_some() {
        super::super::persist::publish(lv);
    }
    value.flatten()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_value_boxes_pass_centred_clicks() {
        create_engine::states::set_range(10, 2);
        let table = StateTable::new(
            10,
            [
                ("create:copper_valve_handle", vec![("facing", "up")]),
                (
                    "create:steam_engine",
                    vec![("face", "wall"), ("facing", "north")],
                ),
            ]
            .into_iter()
            .map(|(name, props)| (name, props.into_iter())),
        );
        for state in [10, 11] {
            let block_at = |_| state;
            for side in Dir::ALL {
                assert!(!value_box_hit(
                    &table,
                    block_at,
                    |_| false,
                    [0; 3],
                    side,
                    false,
                    [0.5; 3]
                ));
                assert!(value_box_slot(&table, block_at, |_| false, [0; 3], side, false).is_none());
            }
        }
        assert!(value_box_hit(
            &table,
            |_| 10,
            |_| false,
            [0; 3],
            Dir::Up,
            true,
            [0.5; 3]
        ));
        create_engine::states::clear_range();
    }
}
