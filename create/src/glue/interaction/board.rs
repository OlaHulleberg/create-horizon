//! The client's side of value boxes: the use press held on one (`ValueSettingsClient`), the
//! board it opens (`ValueSettingsScreen`), the mouse wheel over a block or a board, and the
//! values the server told.

use super::super::items::{self, Item};
use super::super::kinetics;
use super::super::net;
use super::super::states;
use super::crank;
use super::values::{self, SCROLL, VALUE_SETTINGS, may_use_value_box, value_box_hit};
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
    /// An icons board's option names.
    names: &'static [&'static str],
    /// The pick under the cursor: the wheel moves it, as there is no free cursor while playing.
    pick: Settings,
    ticks: u32,
    /// Whether the server told the block's setting (the pick starts on it), and whether the
    /// wheel moved the pick: a board that knows neither sends nothing.
    known: bool,
    moved: bool,
}

/// The use press held on a value box (`ValueSettingsClient.interactHeldPos`,
/// `interactHeldTicks`), and whether the server told its setting yet.
struct Held {
    pos: [i32; 3],
    hold: Hold,
    known: bool,
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

/// The value box under the crosshair, when the player may use it: its block and its board's
/// title.
pub(super) fn hovered_box(host: &dyn ClientHost) -> Option<([i32; 3], &'static str)> {
    let Some(CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return None;
    };
    let pos = [pos.x, pos.y, pos.z];
    let player = host.local_player()?;
    if !may_use_value_box(host, player) || !hits_value_box(host, pos, hit.face, hit.point) {
        return None;
    }
    Some((pos, board_of(host, pos, false)?.board.title))
}

/// Whether the hit `hit` on the block at `pos` lands on its active value box.
fn hits_value_box(host: &dyn ClientHost, pos: [i32; 3], face: [i32; 3], point: [f64; 3]) -> bool {
    let Some(side) = Dir::from_offset(face) else {
        return false;
    };
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    super::super::client_table(host, |t| {
        value_box_hit(
            t,
            |p| block_at(host, p),
            crate::glue::logistics::client::ejector_charged,
            pos,
            side,
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
    if may_use_value_box(&*host, player)
        && hits_value_box(&*host, click.pos, click.hit.face, click.hit.point)
    {
        let idle = HOLD.with_borrow(Option::is_none) && BOARD.with_borrow(Option::is_none);
        if idle {
            HOLD.with_borrow_mut(|h| {
                *h = Some(Held {
                    pos: click.pos,
                    hold: Hold::default(),
                    known: false,
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

/// A board's pick for the block's setting `v` (`getValueSettings`): a motor's speed, an option.
fn board_pick(b: Board, v: i32) -> Settings {
    if b.icons || b == board::EJECTOR {
        Settings { row: 0, value: v }
    } else if b == board::DIODE {
        board::diode_settings(v)
    } else {
        board::kinetic_settings(v)
    }
}

/// The board of the value box at `pos` (`createBoard`), starting on the block's setting when the
/// server told it (`known`), else on what this client sees: the creative motor's speed and a
/// brass tunnel's mode (the chassis' board needs the wrench, whose use turns the block here).
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
        (_, None) => seen.unwrap_or_else(|| {
            create_engine::world::diode_kind(block).map_or(0, |k| k.default_delay())
        }),
    };
    Some(OpenBoard {
        pos,
        board: b,
        names,
        pick: board_pick(b, setting),
        ticks: 0,
        known,
        moved: false,
    })
}

/// The block under the crosshair when the hit lands on its active value box.
fn aimed_value_box(host: &dyn ClientHost) -> Option<[i32; 3]> {
    let Some(CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return None;
    };
    let pos = [pos.x, pos.y, pos.z];
    hits_value_box(host, pos, hit.face, hit.point).then_some(pos)
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
        let [x, y, z] = open.pos;
        let data = values::encode(&[x, y, z, open.pick.row, open.pick.value]);
        net::send_to_server(host, VALUE_SETTINGS, &data);
        return;
    }
    if BOARD.with_borrow(Option::is_some) {
        return;
    }
    let player = host.local_player();
    let aimed = match player {
        Some(p) if may_use_value_box(&*host, p) => aimed_value_box(&*host),
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
            if let Some(open) = board_of(&*host, pos, known) {
                BOARD.with_borrow_mut(|b| *b = Some(open));
            }
        }
        // A short click: `onShortInteract` on the server (`ValueSettingsPacket` with its hand:
        // whether it holds a wrench).
        Some((pos, _, Step::Short)) => {
            HOLD.with_borrow_mut(|h| *h = None);
            let wrench = player.is_some_and(|p| holds_wrench(&*host, p));
            let data = values::encode(&[pos[0], pos[1], pos[2], i32::from(wrench)]);
            net::send_to_server(host, VALUE_SETTINGS, &data);
        }
        // The press is off the box.
        None => HOLD.with_borrow_mut(|h| *h = None),
    }
}

/// The mouse wheel on the client: an open board takes it (`ValueSettingsScreen.mouseScrolled`:
/// a column a step, a milestone with sneak held), else the value under the crosshair: a
/// creative motor's or speed controller's speed, a brass tunnel's mode, and (with the wrench,
/// `requiresWrench`) a chassis' range.
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
                }
            });
        }
        return Flow::Cancel;
    }
    // Create 6 edits values on the board only: the wheel over a block scrolls the hotbar. The
    // chassis' range, whose board (with the wrench) is not ported, keeps the wheel.
    let Some(CrosshairTarget::Block { pos, .. }) = host.crosshair_target() else {
        return Flow::Continue;
    };
    let pos = [pos.x, pos.y, pos.z];
    let wrench = host.local_player().is_some_and(|p| holds_wrench(&*host, p));
    let chassis = block_of(&*host, pos).is_some_and(|b| {
        matches!(
            b,
            B::LinearChassis | B::SecondaryLinearChassis | B::RadialChassis
        )
    });
    if !(wrench && chassis) {
        return Flow::Continue;
    }
    if steps != 0 {
        let data = values::encode(&[pos[0], pos[1], pos[2], steps]);
        net::send_to_server(host, SCROLL, &data);
    }
    Flow::Cancel
}

/// An opaque colour of `rgb`.
fn opaque(rgb: u32) -> u32 {
    0xFF00_0000 | rgb
}

/// `ValueSettingsScreen.renderWindow`, with plain fills where Create draws its GUI sprites
/// (`AllGuiTextures.VALUE_SETTINGS_*`). Whether a board is open.
pub(super) fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext) -> bool {
    let Some((b, names, pick, ticks)) =
        BOARD.with_borrow(|b| b.as_ref().map(|o| (o.board, o.names, o.pick, o.ticks)))
    else {
        return false;
    };
    let [width, height] = draw.size();
    let tw = |draw: &dyn DrawContext, s: &str| draw.text_width(s) as i32;
    let label = b.rows.iter().map(|r| tw(draw, r)).max().unwrap_or(0);
    let l = Layout::new(b, label, [width as i32, height as i32]);
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
    draw.fill([f(fr.x), f(fr.y), f(fr.width), f(fr.height)], 0xE02A_1E14);
    if fr.text {
        let at = [centred(draw, b.title, fr.text_x), f(fr.title_y)];
        draw.text(b.title, at, opaque(0xDD_DDDD), false);
        let at = [centred(draw, &tip, fr.text_x), f(fr.tip_y)];
        draw.text(&tip, at, opaque(0xDD_DDDD), false);
    }
    for (i, row) in b.rows.iter().enumerate() {
        let y = l.top + i as i32 * 11;
        if !b.icons {
            draw.fill([f(l.left - 4), f(y), f(l.label + 8), 11.0], 0xFFC8_A864);
            let bar = [f(l.bar_x()), f(y + 1), f(b.bar_width()), 8.0];
            draw.fill(bar, 0xFF8A_6A3A);
            draw.text(row, [f(l.left), f(y + 1)], opaque(0x44_2000), false);
        }
        for x in l.milestones() {
            let mark = [f(x), f(y + 1), f(b.milestone_size()), 8.0];
            draw.fill(mark, 0xFF44_2000);
        }
    }
    let label = text(pick);
    let width = if b.icons { 16 } else { tw(draw, &label) };
    let [x, y, w] = Layout::cursor(l.coordinate(pick.row, pick.value), width);
    draw.fill([f(x - 3), f(y), f(w + 6), 14.0], 0xFFFB_DC7D);
    if !b.icons {
        draw.text(&label, [f(x + 2), f(y + 3)], opaque(0x44_2000), false);
    } else if fr.text {
        let at = [centred(draw, &label, fr.text_x), f(fr.option_y)];
        draw.text(&label, at, opaque(0xFB_DC7D), false);
    }
    true
}
