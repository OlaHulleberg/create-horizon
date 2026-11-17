//! The client's side of value boxes: the use press held on one (`ValueSettingsClient`), the
//! board it opens (`ValueSettingsScreen`), the mouse wheel over a board, and the
//! values the server told.

use super::super::items::{self, Item};
use super::super::kinetics;
use super::super::net;
use super::super::states;
use super::crank;
use super::values::{self, VALUE_SETTINGS, may_use_value_box, value_box_hit};
use crate::glue::stock::gauge_client as gauge;
use create_engine::Block as B;
use create_engine::board::{self, Board, Hold, Layout, Settings, Step};
use create_engine::kinetics::Dir;
use mod_api::{
    ClientHost, CrosshairTarget, DrawContext, Flow, Hand, InteractionResult, PlayerRef,
    RightClickBlockEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// A value board open on this client (`ValueSettingsScreen`).
struct OpenBoard {
    pos: [i32; 3],
    board: Board,
    /// An icons board's option names, and their icons' cells in `create:gui/icons`.
    names: &'static [&'static str],
    icons: &'static [[f32; 2]],
    /// The pick under the cursor: the wheel moves it, as there is no free cursor while playing.
    pick: Settings,
    ticks: u32,
    /// Whether the server told the block's setting (the pick starts on it), and whether the
    /// wheel or the mouse moved the pick: a board that knows neither sends nothing.
    known: bool,
    moved: bool,
    /// A factory gauge's panel (`FactoryPanelBehaviour`, one of four in its block).
    slot: Option<u8>,
    /// The view when the board opened: it stays there while the board is open, as Create's
    /// screen takes the mouse (`camera_pose`), and the mouse moves `cursor` instead.
    frozen: Option<[f32; 2]>,
    /// The board's cursor (`ValueSettingsScreen`'s mouse), in GUI pixels: placed on the pick
    /// when drawn first (None), moved by the mouse's turn, picking the nearest row and column.
    cursor: Option<[f32; 2]>,
    cursor_moved: bool,
}

/// GUI pixels the board's cursor moves for a degree the mouse would turn the view (vanilla's
/// turn at the default sensitivity, `0.15°` a pixel, at a GUI scale of three).
const CURSOR_PER_DEGREE: f32 = 2.2;

/// The use press held on a value box (`ValueSettingsClient.interactHeldPos`,
/// `interactHeldTicks`), and whether the server told its setting yet.
struct Held {
    pos: [i32; 3],
    hold: Hold,
    known: bool,
    /// A factory gauge's panel.
    slot: Option<u8>,
}

thread_local! {
    static HOLD: RefCell<Option<Held>> = const { RefCell::new(None) };
    static BOARD: RefCell<Option<OpenBoard>> = const { RefCell::new(None) };
    /// Values of the value boxes this client has seen set, by block.
    static VALUES: RefCell<HashMap<[i32; 3], i32>> = RefCell::new(HashMap::new());
}

/// The client left its world.
pub(super) fn reset() {
    HOLD.with_borrow_mut(|h| *h = None);
    BOARD.with_borrow_mut(|b| *b = None);
    VALUES.with_borrow_mut(HashMap::clear);
}

/// The value the server last told for the value box at `pos`.
pub(super) fn seen(pos: [i32; 3]) -> Option<i32> {
    VALUES.with_borrow(|m| m.get(&pos).copied())
}

/// The engine id of the block at `pos` of the level this client shows (0 for any other).
pub(super) fn block_at(host: &dyn ClientHost, pos: [i32; 3]) -> u32 {
    host.level(host.dimension())
        .and_then(|l| l.block_state(host, pos))
        .map_or(0, states::id_or_none)
}

/// The Create block at `pos` this client shows.
pub(super) fn block_of(host: &dyn ClientHost, pos: [i32; 3]) -> Option<B> {
    let state = block_at(host, pos);
    if state == 0 {
        return None;
    }
    super::super::client_table(host, |t| t.get(state).map(|(b, _)| b)).flatten()
}

/// A setting the server sent: the value box and an open board or held press show it.
pub(super) fn values_from_server(data: &[u8]) {
    let w: Vec<i32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| i32::from_le_bytes(*b))
        .collect();
    let [x, y, z, v] = w[..] else {
        return;
    };
    if data.len() != 16 {
        return;
    }
    let pos = [x, y, z];
    VALUES.with_borrow_mut(|m| m.insert(pos, v));
    // The setting a held press or an open board waits for (`getValueSettings`).
    HOLD.with_borrow_mut(|h| {
        if let Some(h) = h.as_mut().filter(|h| h.pos == pos) {
            h.known = true;
        }
    });
    BOARD.with_borrow_mut(|b| {
        if let Some(open) = b.as_mut().filter(|o| o.pos == pos) {
            if !open.moved {
                open.pick = board_pick(open.board, v);
            }
            open.known = true;
        }
    });
}

/// The value box under the crosshair, when the player may use it: its block and its hover tip
/// (its board's title and "Click and hold to edit"; a logistics filter slot's own lines).
pub(super) fn hovered_box(host: &dyn ClientHost) -> Option<([i32; 3], Vec<String>)> {
    let Some(CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return None;
    };
    let pos = [pos.x, pos.y, pos.z];
    let player = host.local_player()?;
    if !may_use_value_box(host, player) {
        return None;
    }
    // `FilteringRenderer.tick`'s tip, with or without its amount board.
    use crate::glue::logistics::amount;
    if amount::client_filter_slot_hit(host, pos, hit.face, hit.point) {
        return Some((pos, amount::client_tip(host, pos)));
    }
    let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
    if crate::glue::stock::shop::client_price_hit(host, pos, local) {
        let title = host
            .translate("create.table_cloth.price_per_order")
            .unwrap_or_else(|| board::TABLE_CLOTH.title.into());
        let hint = host
            .translate("create.gui.value_settings.hold_to_edit")
            .unwrap_or_else(|| board::HOLD_TO_EDIT.into());
        return Some((pos, vec![title, hint]));
    }
    if !hits_value_box(host, pos, hit.face, hit.point) {
        return None;
    }
    let hint = host
        .translate("create.gui.value_settings.hold_to_edit")
        .unwrap_or_else(|| board::HOLD_TO_EDIT.into());
    Some((
        pos,
        vec![board_of(host, pos, false)?.board.title.to_owned(), hint],
    ))
}

/// The amount board's top value when the hit lands on a filter slot that shows one (a brass
/// funnel's or smart chute's, `acceptsValueSettings`) and the main hand holds no wrench or arm
/// (their use is the block's).
fn filter_board_hit(
    host: &dyn ClientHost,
    pos: [i32; 3],
    face: [i32; 3],
    point: [f64; 3],
) -> Option<i32> {
    use crate::glue::logistics::amount;
    let player = host.local_player()?;
    let bypass = items::held(host, player, Hand::Main).is_some_and(|s| {
        matches!(
            s.item().id(host).as_str(),
            "create:wrench" | "create:mechanical_arm"
        )
    });
    if bypass {
        return None;
    }
    // A shop's price tag (`TableClothFilteringBehaviour`).
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    if crate::glue::stock::shop::client_price_hit(host, pos, local) {
        return Some(board::TABLE_CLOTH.max);
    }
    if !amount::client_filter_slot_hit(host, pos, face, point) {
        return None;
    }
    amount::client_shown(host, pos)
}

/// The value box under the crosshair, for its outline: the block, the side it is on and its
/// centre in the block (`ScrollValueRenderer.tick`'s highlighted box).
pub(super) fn hovered_slot(host: &dyn ClientHost) -> Option<([i32; 3], Dir, [f64; 3])> {
    let Some(CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return None;
    };
    let pos = [pos.x, pos.y, pos.z];
    let side = Dir::from_offset(hit.face)?;
    let player = host.local_player()?;
    if !may_use_value_box(host, player) {
        return None;
    }
    if !hits_value_box(host, pos, hit.face, hit.point) {
        return None;
    }
    // Read before borrowing the table: `client_active` borrows it itself.
    let active = values::client_active(host, pos);
    let slot = super::super::client_table(host, |t| {
        super::values::value_box_slot(
            t,
            |p| block_at(host, p),
            crate::glue::logistics::client::ejector_charged,
            pos,
            side,
            active,
        )
    })
    .flatten()?;
    Some((pos, side, slot))
}

/// Whether the hit `hit` on the block at `pos` lands on its active value box.
fn hits_value_box(host: &dyn ClientHost, pos: [i32; 3], face: [i32; 3], point: [f64; 3]) -> bool {
    let Some(side) = Dir::from_offset(face) else {
        return false;
    };
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    let wrench = host.local_player().is_some_and(|p| holds_wrench(host, p));
    // Read before borrowing the table: `client_active` borrows it itself.
    let active = values::client_active(host, pos);
    super::super::client_table(host, |t| {
        super::values::wrench_fits(t, block_at(host, pos), wrench)
            && value_box_hit(
                t,
                |p| block_at(host, p),
                crate::glue::logistics::client::ejector_charged,
                pos,
                side,
                active,
                local,
            )
    }) == Some(true)
}

/// A right click on a block, on the client: `ValueSettingsInputHandler.onBlockActivated`
/// (`startInteractionWith`, and the server asked for the setting the board starts on), and the
/// answer a hand crank's or valve handle's use gives here as on the server.
pub(super) fn right_click(host: &mut dyn ClientHost, event: &mut RightClickBlockEvent<'_>) -> Flow {
    let click = event.click;
    let Some(block) = block_of(&*host, click.pos) else {
        return Flow::Continue;
    };
    let Some(player) = host.player(click.player) else {
        return Flow::Continue;
    };
    // A factory gauge's panel: one with an item holds for its board (the server takes the
    // click of one without, `onShortInteract`).
    if block == B::FactoryGauge
        && may_use_value_box(&*host, player)
        && let Some((slot, set)) = gauge::hit(&*host, click.pos, click.hit.point)
    {
        let idle = HOLD.with_borrow(Option::is_none) && BOARD.with_borrow(Option::is_none);
        if set && idle && click.hand == Hand::Main {
            HOLD.with_borrow_mut(|h| {
                *h = Some(Held {
                    pos: click.pos,
                    hold: Hold::default(),
                    known: true,
                    slot: Some(slot),
                })
            });
        }
        *event.cancellation_result = InteractionResult::Success;
        return Flow::Cancel;
    }
    if may_use_value_box(&*host, player)
        && (hits_value_box(&*host, click.pos, click.hit.face, click.hit.point)
            || filter_board_hit(&*host, click.pos, click.hit.face, click.hit.point).is_some())
    {
        let idle = HOLD.with_borrow(Option::is_none) && BOARD.with_borrow(Option::is_none);
        if idle {
            HOLD.with_borrow_mut(|h| {
                *h = Some(Held {
                    pos: click.pos,
                    hold: Hold::default(),
                    known: false,
                    slot: None,
                })
            });
            net::send_to_server(host, VALUE_SETTINGS, &values::encode(&click.pos));
        }
        *event.cancellation_result = InteractionResult::Success;
        return Flow::Cancel;
    }
    // The block's own use is skipped while sneaking with something in either hand.
    let bypassed = items::sneaking(&*host, player)
        && Hand::BOTH
            .iter()
            .any(|&h| items::held(&*host, player, h).is_some());
    if !bypassed && crank::answers(&*host, player, block) {
        if block == B::HandCrank {
            super::super::sources::handles::cranked(click.pos);
        }
        *event.cancellation_result = InteractionResult::Success;
        return Flow::Cancel;
    }
    Flow::Continue
}

/// The use key while a press on a value box is held or its board is open: taken, with no
/// swing, so the held item is not used and nothing else is clicked (Create's board is a screen,
/// which takes the input while open, and `onBlockActivated` cancels the clicks on the box before
/// it opens).
pub(super) fn interaction_key(event: &mut mod_api::InteractionKeyMappingTriggeredEvent) -> Flow {
    let busy = HOLD.with_borrow(Option::is_some) || BOARD.with_borrow(Option::is_some);
    if event.key != mod_api::InteractionKey::Use || !busy {
        return Flow::Continue;
    }
    event.swing_hand = false;
    Flow::Cancel
}

/// The linear chassis' range board.
const CHASSIS: Board = board::chassis("Range of Sticky Sides", 16);
/// The radial chassis' radius board.
const RADIAL_CHASSIS: Board = board::chassis("Radius when Sticky", 16);

/// A board's pick for the block's setting `v` (`getValueSettings`): a motor's speed, an option.
fn board_pick(b: Board, v: i32) -> Settings {
    if b == CHASSIS || b == RADIAL_CHASSIS {
        board::chassis_settings(v)
    } else if b.icons || b == board::EJECTOR || b == board::TABLE_CLOTH {
        Settings { row: 0, value: v }
    } else if b.title == board::FILTER_TITLE {
        board::filter_settings(v)
    } else if b == board::DIODE {
        board::diode_settings(v)
    } else {
        board::kinetic_settings(v)
    }
}

/// The board of the value box at `pos` (`createBoard`), starting on the block's setting when the
/// server told it (`known`), else on what this client sees: the creative motor's speed and a
/// brass tunnel's mode (a chassis' range board opens with the wrench only).
fn board_of(host: &dyn ClientHost, pos: [i32; 3], known: bool) -> Option<OpenBoard> {
    let state = block_at(host, pos);
    let (block, facing) =
        super::super::client_table(host, |t| t.get(state).map(|(b, i)| (b, i.facing))).flatten()?;
    let seen = seen(pos);
    let (b, names) = match block {
        B::CreativeMotor => (board::MOTOR, &[][..]),
        B::RotationSpeedController => (board::CONTROLLER, &[][..]),
        B::PulseRepeater | B::PulseExtender | B::PulseTimer => (board::DIODE, &[][..]),
        b if create_engine::dye::is_valve_handle(b) => (board::VALVE_HANDLE, &[][..]),
        B::WeightedEjector => (board::EJECTOR, &[][..]),
        // `ChassisScrollValueBehaviour.createBoard` (`contraptions.chassis.range`, a radial
        // chassis' `radius`), ranges 1 to `maxChassisRange`.
        B::LinearChassis | B::SecondaryLinearChassis => (CHASSIS, &[][..]),
        B::RadialChassis => (RADIAL_CHASSIS, &[][..]),
        b if crate::glue::stock::shop::is_cloth(b) => (board::TABLE_CLOTH, &[][..]),
        // `FilteringBehaviour.createBoard`, up to the filter's stack size.
        B::BrassFunnel | B::BrassBeltFunnel | B::SmartChute => {
            let max = crate::glue::logistics::amount::client_shown(host, pos)?;
            (board::filter(max), &[][..])
        }
        // `ScrollOptionBehaviour`s: a brass tunnel's `selectionMode`
        // (`logistics.when_multiple_outputs_available`), an arm's, the contraption blocks' modes.
        _ => {
            let (title, names) = values::option_names(block)?;
            (board::options(title, names.len() as i32), names)
        }
    };
    let setting = match (block, seen.filter(|_| known)) {
        (_, Some(v)) => v,
        (B::CreativeMotor, None) => {
            // The speed the network turns it at (`getTheoreticalSpeed`, an overstressed
            // network's too).
            let speed = host
                .level(host.dimension())
                .map_or(0.0, |l| kinetics::shown_theory(host, l, pos).0);
            facing.map_or(speed, |f| {
                create_engine::kinetics::convert_to_direction(speed, f)
            }) as i32
        }
        // `SpeedControllerBlockEntity.DEFAULT_SPEED` until the server tells its target.
        (B::RotationSpeedController, None) => seen.unwrap_or(16),
        // The angle its block entity's data brought (`angleInput`).
        (b, None) if create_engine::dye::is_valve_handle(b) => seen.unwrap_or_else(|| {
            host.level(host.dimension())
                .and_then(|l| host.block_entity(l, pos))
                .and_then(|e| e.data(host))
                .map_or(create_engine::valve::DEFAULT_ANGLE, |d| {
                    super::super::sources::handles::valve_angle(&d)
                })
        }),
        // Round robin and any stack size, until the server says.
        (B::MechanicalArm | B::WeightedEjector, None) => 0,
        // Up to a stack until the server says.
        (b, None) if crate::glue::stock::shop::is_cloth(b) => {
            crate::glue::stock::shop::client_price_count(host, pos).unwrap_or(1)
        }
        (B::BrassFunnel | B::BrassBeltFunnel | B::SmartChute, None) => {
            board::FilterAmount::default().word(b.max)
        }
        (_, None) => seen.unwrap_or_else(|| {
            create_engine::world::diode_kind(block).map_or(0, |k| k.default_delay())
        }),
    };
    Some(OpenBoard {
        pos,
        board: b,
        names,
        icons: values::option_icons(block),
        pick: board_pick(b, setting),
        ticks: 0,
        known,
        moved: false,
        slot: None,
        frozen: None,
        cursor: None,
        cursor_moved: false,
    })
}

/// While a board is open the camera keeps the view it opened on (`camera_pose`).
pub(super) fn camera_pose(event: &mut mod_api::CameraPoseEvent<'_>) {
    if let Some([yaw, pitch]) = BOARD.with_borrow(|b| b.as_ref().and_then(|o| o.frozen)) {
        event.yaw = yaw;
        event.pitch = pitch;
    }
}

/// The mouse on an open board (`ValueSettingsScreen`'s cursor): the player's turn since the
/// last tick moves the board's cursor, and the view goes back to where it was.
fn track_mouse(host: &mut dyn ClientHost) {
    let Some(player) = host.local_player() else {
        return;
    };
    let Some([yaw, pitch]) = player.entity().rotation(&*host) else {
        return;
    };
    let frozen = BOARD.with_borrow_mut(|b| {
        let open = b.as_mut()?;
        let [fy, fp] = *open.frozen.get_or_insert([yaw, pitch]);
        let (dx, dy) = (yaw - fy, pitch - fp);
        if (dx != 0.0 || dy != 0.0)
            && let Some(c) = open.cursor.as_mut()
        {
            c[0] += dx * CURSOR_PER_DEGREE;
            c[1] += dy * CURSOR_PER_DEGREE;
            open.cursor_moved = true;
        }
        Some([fy, fp])
    });
    if let Some([fy, fp]) = frozen
        && (fy != yaw || fp != pitch)
    {
        let _ = host.set_local_rotation(fy, fp);
    }
}

/// The block under the crosshair when the hit lands on its active value box.
fn aimed_value_box(host: &dyn ClientHost) -> Option<[i32; 3]> {
    let Some(CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return None;
    };
    let pos = [pos.x, pos.y, pos.z];
    let hit = hits_value_box(host, pos, hit.face, hit.point)
        || filter_board_hit(host, pos, hit.face, hit.point).is_some();
    hit.then_some(pos)
}

/// Whether the vanilla mapping `name` is held.
fn mapping_down(host: &dyn ClientHost, name: &str) -> bool {
    host.mapping(name).ok().flatten().is_some_and(|m| m.down)
}

/// Whether the client's player holds the wrench in the main hand.
pub(super) fn holds_wrench(host: &dyn ClientHost, player: PlayerRef) -> bool {
    items::is(items::held(host, player, Hand::Main).as_ref(), Item::Wrench)
}

/// `ValueSettingsClient.tick` and the board's release (`ValueSettingsScreen.saveAndClose`): a
/// hold goes on while the player may interact and aims at its box; on its sixth tick the board
/// opens. Letting go of use sends the pick.
pub(super) fn tick(host: &mut dyn ClientHost) {
    // Hot: nothing runs while no press is held and no board is open.
    if HOLD.with_borrow(Option::is_none) && BOARD.with_borrow(Option::is_none) {
        return;
    }
    let down = mapping_down(&*host, "key.use");
    if down && BOARD.with_borrow(Option::is_some) {
        track_mouse(host);
    }
    let released = BOARD.with_borrow_mut(|b| {
        let open = b.as_mut()?;
        open.ticks += 1;
        if down { None } else { b.take() }
    });
    if let Some(open) = released {
        // Never a guess: unmoved and with no answer from the server, nothing changes.
        if !open.known && !open.moved {
            return;
        }
        if let Some(slot) = open.slot {
            gauge::send_amount(host, open.pos, slot, open.pick.row, open.pick.value);
            return;
        }
        let [x, y, z] = open.pos;
        let data = values::encode(&[x, y, z, open.pick.row, open.pick.value]);
        net::send_to_server(host, VALUE_SETTINGS, &data);
        return;
    }
    if BOARD.with_borrow(Option::is_some) {
        return;
    }
    let player = host.local_player();
    let gauge_slot = HOLD.with_borrow(|h| h.as_ref().and_then(|h| h.slot));
    let aimed = match player {
        Some(p) if may_use_value_box(&*host, p) => match gauge_slot {
            // The same panel still under the crosshair.
            Some(slot) => match host.crosshair_target() {
                Some(CrosshairTarget::Block { pos, hit, .. }) => {
                    let pos = [pos.x, pos.y, pos.z];
                    (gauge::hit(&*host, pos, hit.point).map(|h| h.0) == Some(slot)).then_some(pos)
                }
                _ => None,
            },
            None => aimed_value_box(&*host),
        },
        _ => None,
    };
    // `cancelInteraction` when the player may no longer interact or looks off the box.
    let step = HOLD.with_borrow_mut(|h| {
        let held = h.as_mut().filter(|held| Some(held.pos) == aimed)?;
        Some((held.pos, held.known, held.hold.tick(down)))
    });
    match step {
        Some((_, _, Step::Wait)) => {}
        Some((pos, known, Step::Open)) => {
            HOLD.with_borrow_mut(|h| *h = None);
            if let Some(slot) = gauge_slot {
                // `FactoryPanelBehaviour.createBoard`, on its `getValueSettings`.
                let (row, value) = gauge::board_pick(&*host, pos, slot);
                let open = OpenBoard {
                    pos,
                    board: board::FACTORY,
                    names: &[],
                    icons: &[],
                    pick: Settings { row, value },
                    ticks: 0,
                    known: true,
                    moved: false,
                    slot: Some(slot),
                    frozen: None,
                    cursor: None,
                    cursor_moved: false,
                };
                BOARD.with_borrow_mut(|b| *b = Some(open));
            } else if let Some(open) = board_of(&*host, pos, known) {
                BOARD.with_borrow_mut(|b| *b = Some(open));
            }
        }
        // A short click: `onShortInteract` on the server (`ValueSettingsPacket` with its hand:
        // whether it holds a wrench).
        Some((pos, _, Step::Short)) => {
            HOLD.with_borrow_mut(|h| *h = None);
            let wrench = player.is_some_and(|p| holds_wrench(&*host, p));
            if let Some(slot) = gauge_slot {
                gauge::short_click(host, pos, slot, wrench);
                return;
            }
            let data = values::encode(&[pos[0], pos[1], pos[2], i32::from(wrench)]);
            net::send_to_server(host, VALUE_SETTINGS, &data);
        }
        // The press is off the box.
        None => HOLD.with_borrow_mut(|h| *h = None),
    }
}

/// The mouse wheel on the client: an open board takes it (`ValueSettingsScreen.mouseScrolled`:
/// a column a step, a milestone with sneak held); over a block it scrolls the hotbar.
pub(super) fn wheel(host: &mut dyn ClientHost, steps: i32) -> Flow {
    if BOARD.with_borrow(Option::is_some) {
        if steps != 0 {
            let shift = mapping_down(&*host, "key.sneak");
            BOARD.with_borrow_mut(|b| {
                if let Some(open) = b.as_mut()
                    && let Some(column) = open.board.scrolled(open.pick, steps > 0, shift)
                {
                    open.pick.value = column;
                    open.moved = true;
                    // `setCursor(getCoordinateOfValue(..))`: the cursor follows.
                    open.cursor = None;
                }
            });
        }
        return Flow::Cancel;
    }
    // Create 6 edits values on the board only: the wheel over a block scrolls the hotbar.
    Flow::Continue
}

/// An opaque colour of `rgb`.
fn opaque(rgb: u32) -> u32 {
    0xFF00_0000 | rgb
}

/// `AllGuiTextures.VALUE_SETTINGS_*` and `BRASS_FRAME_*`.
const TEX: &str = "create:gui/value_settings";

/// `UIRenderHelper.drawStretched`: texture part `src` stretched over `rect`.
fn stretched(draw: &mut dyn DrawContext, src: [f32; 4], rect: [f32; 4]) {
    draw.sprite(TEX, src, rect, None);
}

/// `UIRenderHelper.drawCropped` (and `AllGuiTextures.render`): the top left `w`x`h` of the
/// texture part at `src` drawn at `at`.
fn cropped(draw: &mut dyn DrawContext, src: [f32; 2], at: [f32; 2], w: f32, h: f32) {
    draw.sprite(TEX, [src[0], src[1], w, h], [at[0], at[1], w, h], None);
}

/// `ValueSettingsScreen.renderBrassFrame`.
fn brass_frame(draw: &mut dyn DrawContext, x: f32, y: f32, w: f32, h: f32) {
    cropped(draw, [65.0, 9.0], [x, y], 4.0, 4.0);
    cropped(draw, [70.0, 9.0], [x + w - 4.0, y], 4.0, 4.0);
    cropped(draw, [65.0, 19.0], [x, y + h - 4.0], 4.0, 4.0);
    cropped(draw, [70.0, 19.0], [x + w - 4.0, y + h - 4.0], 4.0, 4.0);
    if h > 8.0 {
        stretched(draw, [65.0, 14.0, 3.0, 4.0], [x, y + 4.0, 3.0, h - 8.0]);
        stretched(
            draw,
            [71.0, 14.0, 3.0, 4.0],
            [x + w - 3.0, y + 4.0, 3.0, h - 8.0],
        );
    }
    if w > 8.0 {
        cropped(draw, [0.0, 24.0], [x + 4.0, y], w - 8.0, 3.0);
        cropped(draw, [0.0, 27.0], [x + 4.0, y + h - 3.0], w - 8.0, 3.0);
    }
}

/// `ValueSettingsScreen.renderWindow`. Whether a board is open.
pub(super) fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext) -> bool {
    let Some((b, names, icons, ticks)) =
        BOARD.with_borrow(|b| b.as_ref().map(|o| (o.board, o.names, o.icons, o.ticks)))
    else {
        return false;
    };
    let [width, height] = draw.size();
    let tw = |draw: &dyn DrawContext, s: &str| draw.text_width(s) as i32;
    let label = b.rows.iter().map(|r| tw(draw, r)).max().unwrap_or(0);
    let l = Layout::new(b, label, [width as i32, height as i32]);
    // The cursor starts on the pick; once the mouse moves it, the pick is the nearest row and
    // column to it (`getClosestCoordinate`, milestones only with shift held).
    let shift = mapping_down(host, "key.sneak");
    let pick = BOARD.with_borrow_mut(|o| {
        let open = o.as_mut()?;
        match open.cursor {
            None => open.cursor = Some(l.coordinate(open.pick.row, open.pick.value)),
            Some(c) if open.cursor_moved => {
                let closest = l.closest([c[0] as i32, c[1] as i32], shift);
                // Kept within the board, so moving back comes back at once.
                let last = b.rows.len() as i32 - 1;
                let [x0, y0] = l.coordinate(0, 0);
                let [x1, y1] = l.coordinate(last.max(0), b.max);
                open.cursor = Some([
                    c[0].clamp(x0 - 4.0, x1 + 4.0),
                    c[1].clamp(y0 - 6.0, y1 + 6.0),
                ]);
                if closest != open.pick {
                    open.pick = closest;
                    open.moved = true;
                }
                open.cursor_moved = false;
            }
            Some(_) => {}
        }
        Some(open.pick)
    });
    let Some(pick) = pick else {
        return false;
    };
    let key = host
        .mapping("key.use")
        .ok()
        .flatten()
        .and_then(|m| host.text_string(&m.label).ok())
        .unwrap_or_else(|| "Use".to_owned());
    let tip = board::release_to_confirm(&key);
    let text = |s: Settings| {
        if b.icons {
            names
                .get(s.value.clamp(0, names.len() as i32 - 1) as usize)
                .copied()
                .unwrap_or_default()
                .to_owned()
        } else if b == board::EJECTOR {
            board::ejector_text(s)
        } else if b == CHASSIS || b == RADIAL_CHASSIS {
            board::chassis_text(s)
        } else if b == board::FACTORY {
            board::factory_text(s)
        } else if b.title == board::FILTER_TITLE {
            board::filter_text(s, b.max)
        } else if b == board::DIODE {
            board::diode_text(s)
        } else if b == board::VALVE_HANDLE {
            board::valve_text(s)
        } else {
            board::kinetic_text(s)
        }
    };
    let mut fattest = tw(draw, &tip).max(tw(draw, b.title));
    if b.icons {
        for value in 0..=b.max {
            fattest = fattest.max(tw(draw, &text(Settings { row: 0, value })));
        }
    }
    let f = |v: i32| v as f32;
    let centred = |draw: &dyn DrawContext, text: &str, x: i32| f(x - tw(draw, text) / 2);
    let open = ticks as f32;
    draw.fill([0.0, 0.0, width, height], board::shade(open));
    let fr = l.frame(fattest, open);
    // The background, and its top edge a pixel narrower each side. (Create's bottom edge passes
    // its width and height in the wrong places and draws nothing.)
    const OUTER_BG: [f32; 4] = [80.0, 9.0, 1.0, 1.0];
    stretched(
        draw,
        OUTER_BG,
        [f(fr.x), f(fr.y), f(fr.width), f(fr.height)],
    );
    stretched(
        draw,
        OUTER_BG,
        [f(fr.x + 1), f(fr.y - 1), f(fr.width - 2), 1.0],
    );
    if fr.text {
        let at = [centred(draw, b.title, fr.text_x), f(fr.title_y)];
        draw.text(b.title, at, opaque(0xDD_DDDD), false);
        let at = [centred(draw, &tip, fr.text_x), f(fr.tip_y)];
        draw.text(&tip, at, opaque(0xDD_DDDD), false);
    }
    let rows = b.rows.len() as i32;
    brass_frame(
        draw,
        f(l.left + l.label + 14),
        f(l.top - 3),
        f(b.bar_width() + 8),
        f(rows * 11 + 5),
    );
    stretched(
        draw,
        [75.0, 9.0, 1.0, 1.0],
        [
            f(l.left + l.label + 17),
            f(l.top),
            f(b.bar_width() + 2),
            f(rows * 11 - 1),
        ],
    );
    for (i, row) in b.rows.iter().enumerate() {
        let y = l.top + i as i32 * 11;
        if !b.icons {
            cropped(
                draw,
                [0.0, 31.0],
                [f(l.left - 4), f(y)],
                f(l.label + 8),
                11.0,
            );
            let mut w = 0;
            while w < b.bar_width() {
                let part = 248.min(b.bar_width() - w);
                cropped(draw, [7.0, 0.0], [f(l.bar_x() + w), f(y + 1)], f(part), 8.0);
                w += 248;
            }
            draw.text(row, [f(l.left), f(y + 1)], opaque(0x44_2000), false);
        }
        for x in l.milestones() {
            if b.icons {
                cropped(draw, [75.0, 14.0], [f(x), f(y + 1)], 13.0, 8.0);
            } else {
                cropped(draw, [0.0, 0.0], [f(x), f(y + 1)], 7.0, 8.0);
            }
        }
    }
    if !b.icons {
        brass_frame(
            draw,
            f(l.left - 7),
            f(l.top - 3),
            f(l.label + 14),
            f(rows * 11 + 5),
        );
    }
    if ticks < 1 {
        return true;
    }
    let label = text(pick);
    let icon = b
        .icons
        .then(|| icons.get(pick.value.max(0) as usize).copied())
        .flatten();
    let width = if icon.is_some() { 16 } else { tw(draw, &label) };
    let [x, y, w] = Layout::cursor(l.coordinate(pick.row, pick.value), width);
    if let Some(icon) = icon {
        cropped(draw, [0.0, 44.0], [f(x - 2), f(y - 3)], 22.0, 20.0);
        // The icon in the cursor's brown (`setShaderColor(0.265625, 0.125, 0, 1)`).
        draw.sprite(
            "create:gui/icons",
            [icon[0], icon[1], 16.0, 16.0],
            [f(x + 1), f(y - 1), 16.0, 16.0],
            Some(0xFF44_2000),
        );
        if fr.text {
            let at = [centred(draw, &label, fr.text_x), f(fr.option_y)];
            draw.text(&label, at, opaque(0xFB_DC7D), false);
        }
        return true;
    }
    cropped(draw, [0.0, 9.0], [f(x - 3), f(y)], 3.0, 14.0);
    cropped(draw, [4.0, 9.0], [f(x), f(y)], f(w), 14.0);
    cropped(draw, [61.0, 9.0], [f(x + w), f(y)], 3.0, 14.0);
    draw.text(&label, [f(x + 2), f(y + 3)], opaque(0x44_2000), false);
    true
}
