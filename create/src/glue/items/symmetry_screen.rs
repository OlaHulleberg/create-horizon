//! The wand of symmetry's screen (`SymmetryWandScreen`): the mirror's type and orientation as
//! two selection inputs, a preview of the mirror's model, the wand's name, the wand itself and
//! the confirm button. It opens on the client when the wand is used sneaking
//! (`SymmetryWandItem.use` / `useOn`), from the next client tick; closing it sends the mirror to
//! the server (`ConfigureSymmetryWandPacket`), which writes it into the wand still in that
//! hand.
//!
//! The wand Create draws four times its size, turned, right of the window (`GuiGameElement` with
//! a rotation) is drawn flat at that size: a screen's item has no rotation.

use super::gadget_gui::{
    self as gui, CURIOSITIES, I_CONFIRM, corner, icon_button, inside, selection_tooltip, tr,
};
use crate::glue::net;
use create_engine::symmetry::Kind;
use mod_api::{
    ClientHost, Flow, Hand, ItemStack, SceneBlock, SceneView, ScreenClosingEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
};
use std::cell::RefCell;

/// Client to server: the mirror chosen (`ConfigureSymmetryWandPacket`).
pub(super) const CHANNEL: &str = "create:configure_symmetry_wand";
const MOD: &str = "create";

/// `AllGuiTextures.WAND_OF_SYMMETRY` and `setWindowOffset(-20, 0)`.
const SIZE: [f32; 2] = [188.0, 101.0];
const OFFSET_X: i32 = -20;
/// The inputs (`SelectionScrollInput`s), `[x, y, w, h]` from the window's corner.
const TYPE_INPUT: [f32; 4] = [45.0, 21.0, 109.0, 18.0];
const ALIGN_INPUT: [f32; 4] = [45.0, 43.0, 109.0, 18.0];
const CONFIRM: [f32; 2] = [SIZE[0] - 33.0, SIZE[1] - 24.0];

/// What the screen sends: the hand the wand is in, the mirror's type and orientation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Configure {
    pub hand: Hand,
    pub kind: Kind,
    pub orientation: usize,
}

impl Configure {
    pub(super) fn encode(&self) -> Vec<u8> {
        let kind = Kind::ALL.iter().position(|k| *k == self.kind).unwrap_or(0);
        vec![
            u8::from(self.hand == Hand::Off),
            kind as u8,
            self.orientation.min(255) as u8,
        ]
    }

    pub(super) fn decode(data: &[u8]) -> Option<Configure> {
        let [hand, kind, orientation] = data else {
            return None;
        };
        Some(Configure {
            hand: if *hand == 1 { Hand::Off } else { Hand::Main },
            kind: *Kind::ALL.get(usize::from(*kind))?,
            orientation: usize::from(*orientation),
        })
    }
}

struct Open {
    wand: ItemStack,
    configure: Configure,
    size: [f32; 2],
    pressed: bool,
}

thread_local! {
    /// A sneaking use asked for the screen: it opens at the next client tick.
    static PENDING: RefCell<Option<(ItemStack, Hand)>> = const { RefCell::new(None) };
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// Mirrors to send, and whether to close the screen, at the next client tick.
    static LATER: RefCell<(Vec<Configure>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub(super) fn reset() {
    PENDING.with_borrow_mut(|p| *p = None);
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

/// `openWandGUI(wand, hand)`.
pub(super) fn request(wand: ItemStack, hand: Hand) {
    PENDING.with_borrow_mut(|p| *p = Some((wand, hand)));
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if close {
        host.mod_screen_close(MOD);
    }
    for c in send {
        net::send_to_server(host, CHANNEL, &c.encode());
    }
    // Closed some other way (another screen took its place): `removed` still sends.
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        net::send_to_server(host, CHANNEL, &open.configure.encode());
    }
    let Some((wand, hand)) = PENDING.with_borrow_mut(Option::take) else {
        return;
    };
    // The constructor: an empty mirror is shown as a plane.
    let mirror = super::symmetry::wand(&wand);
    let (kind, orientation) = match mirror.kind {
        Kind::Empty => (Kind::Plane, 0),
        k => (k, mirror.orientation),
    };
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            wand,
            configure: Configure {
                hand,
                kind,
                orientation,
            },
            size: [0.0, 0.0],
            pressed: false,
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn at(r: [f32; 4], x: f32, y: f32) -> [f32; 4] {
    [x + r[0], y + r[1], r[2], r[3]]
}

fn type_names(host: &dyn ClientHost) -> Vec<String> {
    Kind::SCREEN
        .iter()
        .map(|k| tr(host, k.screen_key()))
        .collect()
}

fn align_names(host: &dyn ClientHost, kind: Kind) -> Vec<String> {
    kind.align_keys().iter().map(|k| tr(host, k)).collect()
}

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let mouse = event.screen.mouse;
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut() {
            o.size = size;
            draw(host, event.draw, o, mouse);
        }
    });
}

/// `renderWindow` and the widgets.
fn draw(host: &dyn ClientHost, draw: &mut dyn mod_api::DrawContext, o: &Open, mouse: [f32; 2]) {
    super::gadget_gui::dim(host, draw);
    let (x, y) = corner(draw.size(), SIZE, OFFSET_X);
    draw.sprite(
        CURIOSITIES,
        [0.0, 131.0, SIZE[0], SIZE[1]],
        [x, y, SIZE[0], SIZE[1]],
        None,
    );
    let name = host.hover_name(&o.wand);
    let w = draw.text_width(&name) as i32;
    draw.text(
        &name,
        [x + ((SIZE[0] as i32 - w) / 2) as f32, y + 4.0],
        0xFF59_2424,
        false,
    );
    // `renderBlock`: the mirror's model, turned 22.5° about (0.3, 1, 0) and by its own
    // orientation, sixteen pixels a block, standing on (26, 39).
    let c = o.configure;
    if let Some((_, turn)) = c.kind.model(c.orientation) {
        let part = match c.kind {
            Kind::CrossPlane => "create:part_symmetry_crossplane[facing=up]",
            Kind::TriplePlane => "create:part_symmetry_tripleplane[facing=up]",
            _ => "create:part_symmetry_plane[facing=up]",
        };
        if let Some(state) = host.block_state_id(part) {
            let _ = draw.draw_scene(
                [x + 18.0, y + 15.0, x + 50.0, y + 47.0],
                &mod_api::Scene::new(vec![SceneBlock::new([0, 0, 0], state, None)]),
                SceneView::new([0.5, 0.5, 0.5], turn - 22.5, 6.5, 16.0),
            );
        }
    }
    // The labels the inputs write to.
    let types = type_names(host);
    let state = Kind::SCREEN.iter().position(|k| *k == c.kind).unwrap_or(0);
    draw.text(&types[state], [x + 51.0, y + 28.0], 0xFFFF_FFFF, true);
    let aligns = align_names(host, c.kind);
    if let Some(a) = aligns.get(c.orientation) {
        draw.text(a, [x + 51.0, y + 50.0], 0xFFFF_FFFF, true);
    }
    icon_button(
        draw,
        [x + CONFIRM[0], y + CONFIRM[1]],
        I_CONFIRM,
        false,
        mouse,
        o.pressed,
    );
    // The wand, four times its size, right of the window.
    draw.item_stack(&o.wand, [x + 178.0, y + 45.0], 64.0);
    // The hovered input's tooltip.
    let tip = if inside(at(TYPE_INPUT, x, y), mouse) {
        Some(selection_tooltip(
            host,
            tr(host, "gui.symmetryWand.mirrorType"),
            &types,
            state,
        ))
    } else if inside(at(ALIGN_INPUT, x, y), mouse) {
        Some(selection_tooltip(
            host,
            tr(host, "gui.symmetryWand.orientation"),
            &aligns,
            c.orientation,
        ))
    } else {
        None
    };
    if let Some(lines) = tip {
        let _ = draw.gui_tooltip(&lines, mouse);
    }
}

pub(super) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let wheel = event.scroll[1];
    if wheel == 0.0 {
        return Flow::Cancel;
    }
    let mouse = event.screen.mouse;
    let shift = gui::shift(event.modifiers);
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let (x, y) = corner(o.size, SIZE, OFFSET_X);
        let c = &mut o.configure;
        if inside(at(TYPE_INPUT, x, y), mouse) {
            let state = Kind::SCREEN.iter().position(|k| *k == c.kind).unwrap_or(0) as i32;
            if let Some(next) = gui::scroll(host, state, (0, 3), wheel, true, shift) {
                // A new mirror of that type at the same place, in its first orientation.
                c.kind = Kind::SCREEN[next as usize];
                c.orientation = 0;
            }
        } else if inside(at(ALIGN_INPUT, x, y), mouse) {
            let n = c.kind.align_keys().len() as i32;
            if let Some(next) = gui::scroll(host, c.orientation as i32, (0, n), wheel, true, shift)
            {
                c.orientation = next as usize;
            }
        }
    });
    Flow::Cancel
}

pub(super) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let confirmed = OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return false;
        };
        o.pressed = event.button == 0;
        let (x, y) = corner(o.size, SIZE, OFFSET_X);
        event.button == 0 && inside(gui::square([x + CONFIRM[0], y + CONFIRM[1]]), mouse)
    });
    if confirmed {
        gui::click_sound(host);
        // `onClose`, then `removed` sends the mirror.
        if let Some(open) = OPEN.with_borrow_mut(Option::take) {
            LATER.with_borrow_mut(|l| {
                l.0.push(open.configure);
                l.1 = true;
            });
        }
    }
    Flow::Cancel
}

pub(super) fn screen_mouse_released(
    host: &dyn ClientHost,
    _event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut() {
            o.pressed = false;
        }
    });
    Flow::Cancel
}

/// `removed`: the mirror goes to the server at the next tick.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| l.0.push(open.configure));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mirror_crosses_the_wire() {
        let c = Configure {
            hand: Hand::Off,
            kind: Kind::CrossPlane,
            orientation: 1,
        };
        assert_eq!(Configure::decode(&c.encode()), Some(c));
        assert_eq!(Configure::decode(&[0, 9, 0]), None);
        assert_eq!(Configure::decode(&[0, 1]), None);
    }
}
