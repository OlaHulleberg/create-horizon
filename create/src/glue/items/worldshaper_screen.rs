//! The handheld worldshaper's screen (`WorldshaperScreen` on `ZapperScreen`): the brush and its
//! parameters as scroll inputs, the tools the brush takes, the placement options, the follow
//! diagonals and ignore material borders toggles of the surface and cluster brushes, the six
//! placement patterns, the block it paints with, the worldshaper itself and the confirm button.
//! It opens on the client when the worldshaper is used sneaking (`ZapperItem.use` / `useOn`),
//! from the next client tick; closing it sends the settings to the server
//! (`ConfigureWorldshaperPacket`), which writes them into the worldshaper still in that hand.
//!
//! The block Create draws turned 25° and 45° (`renderBlock`) is drawn as a scene of that one
//! block at the same angles.

use super::gadget_gui::{
    self as gui, CURIOSITIES, I_CONFIRM, corner, icon_button, indicator, inside, scroll_tooltip,
    selection_tooltip, square, tr,
};
use crate::glue::net;
use create_engine::worldshaper::{self as ws, Brush, Pattern, Placement, Tool};
use mod_api::{
    ClientHost, DrawContext, Flow, Hand, ItemStack, SceneBlock, SceneView, ScreenClosingEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, Text,
};
use std::cell::RefCell;

/// Client to server: the settings chosen (`ConfigureWorldshaperPacket`).
pub(super) const CHANNEL: &str = "create:configure_worldshaper";
const MOD: &str = "create";

/// `AllGuiTextures.TERRAINZAPPER` and `setWindowOffset(-10, 0)`.
const SIZE: [f32; 2] = [234.0, 103.0];
const OFFSET_X: i32 = -10;
const BRUSH_INPUT: [f32; 4] = [56.0, 20.0, 77.0, 18.0];
const CONFIRM: [f32; 2] = [SIZE[0] - 33.0, SIZE[1] - 24.0];
/// `WorldshaperScreen`'s `fontColor`, and the title's (`ZapperScreen.drawOnBackground`).
const FONT: u32 = 0xFF76_7676;
const TITLE: u32 = 0xFF54_214F;

/// The brushes in the screen's order (`TerrainBrushes.values()`).
const BRUSHES: [Brush; 5] = [
    Brush::Cuboid,
    Brush::Sphere,
    Brush::Cylinder,
    Brush::Surface,
    Brush::Cluster,
];
const PATTERNS: [Pattern; 6] = [
    Pattern::Solid,
    Pattern::Checkered,
    Pattern::InverseCheckered,
    Pattern::Chance25,
    Pattern::Chance50,
    Pattern::Chance75,
];
const PLACEMENTS: [Placement; 3] = [Placement::Merged, Placement::Attached, Placement::Inserted];

/// `AllIcons` cells.
fn tool_icon(t: Tool) -> [f32; 2] {
    let i = match t {
        Tool::Fill => 0.0,
        Tool::Place => 1.0,
        Tool::Replace => 2.0,
        Tool::Clear => 3.0,
        Tool::Overlay => 4.0,
        Tool::Flatten => 5.0,
    };
    [112.0 + 16.0 * i, 32.0]
}

fn pattern_icon(p: Pattern) -> [f32; 2] {
    match p {
        Pattern::Solid => [0.0, 96.0],
        Pattern::Checkered => [16.0, 96.0],
        Pattern::InverseCheckered => [32.0, 96.0],
        Pattern::Chance25 => [48.0, 96.0],
        Pattern::Chance50 => [0.0, 112.0],
        Pattern::Chance75 => [16.0, 112.0],
    }
}

fn placement_icon(p: Placement) -> [f32; 2] {
    match p {
        Placement::Merged => [64.0, 32.0],
        Placement::Attached => [80.0, 32.0],
        Placement::Inserted => [96.0, 32.0],
    }
}

const I_FOLLOW_DIAGONAL: [f32; 2] = [32.0, 112.0];
const I_FOLLOW_MATERIAL: [f32; 2] = [48.0, 112.0];

/// `translationKey`s under `create.gui.terrainzapper.`.
fn tool_key(t: Tool) -> &'static str {
    t.name()
}

fn pattern_key(p: Pattern) -> &'static str {
    match p {
        Pattern::InverseCheckered => "inversecheckered",
        p => p.name(),
    }
}

fn index<T: PartialEq>(all: &[T], v: T) -> u8 {
    all.iter().position(|a| *a == v).unwrap_or(0) as u8
}

/// What the screen sends (`ConfigureWorldshaperPacket`): the hand, then the settings
/// (`params` with the connectivity toggles folded in, as `getConfigurationPacket` does).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Configure {
    pub hand: Hand,
    pub settings: ws::Settings,
}

impl Configure {
    pub(super) fn encode(&self) -> Vec<u8> {
        let s = &self.settings;
        let mut out = vec![
            u8::from(self.hand == Hand::Off),
            index(&PATTERNS, s.pattern),
            index(&BRUSHES, s.brush),
            index(Tool::ALL, s.tool),
            index(&PLACEMENTS, s.placement),
        ];
        for p in s.params.unwrap_or([1, 1, 1]) {
            out.extend_from_slice(&p.to_le_bytes());
        }
        out
    }

    pub(super) fn decode(data: &[u8]) -> Option<Configure> {
        if data.len() != 17 {
            return None;
        }
        let param = |i: usize| {
            let at = 5 + 4 * i;
            i32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
        };
        Some(Configure {
            hand: if data[0] == 1 { Hand::Off } else { Hand::Main },
            settings: ws::Settings {
                pattern: *PATTERNS.get(usize::from(data[1]))?,
                brush: *BRUSHES.get(usize::from(data[2]))?,
                tool: *Tool::ALL.get(usize::from(data[3]))?,
                placement: *PLACEMENTS.get(usize::from(data[4]))?,
                params: Some([param(0), param(1), param(2)]),
            },
        })
    }
}

/// The screen's state (`WorldshaperScreen`'s fields).
struct Open {
    shaper: ItemStack,
    hand: Hand,
    brush: Brush,
    /// `currentBrushParams`.
    params: [i32; 3],
    diagonals: bool,
    across: bool,
    tool: Tool,
    placement: Placement,
    pattern: Pattern,
    /// The block it paints with, for the preview.
    block: Option<String>,
    size: [f32; 2],
    pressed: bool,
}

impl Open {
    /// The constructor: the stack's settings, `{1, 1, 1}` without params; a dynamic brush's
    /// second and third params at 0 are its toggles on.
    fn new(shaper: ItemStack, hand: Hand) -> Open {
        let (s, block) = super::worldshaper::shaper(&shaper);
        let mut o = Open {
            shaper,
            hand,
            brush: s.brush,
            params: s.params.unwrap_or([1, 1, 1]),
            diagonals: false,
            across: false,
            tool: s.tool,
            placement: s.placement,
            pattern: s.pattern,
            block,
            size: [0.0, 0.0],
            pressed: false,
        };
        if s.params.is_some() {
            o.diagonals = o.params[1] == 0;
            o.across = o.params[2] == 0;
        }
        o.brush_changed();
        o
    }

    /// `initBrushParams`: each input's state kept in the brush's range (`setState` then
    /// `onChanged`), and a tool the brush doesn't take replaced by its first.
    fn brush_changed(&mut self) {
        for (i, p) in self.params.iter_mut().enumerate() {
            let (lo, hi) = self.brush.bounds(i);
            *p = (*p).clamp(lo, hi);
        }
        let tools = self.brush.tools();
        if !tools.contains(&self.tool) {
            self.tool = tools[0];
        }
    }

    /// `getConfigurationPacket`.
    fn configure(&self) -> Configure {
        let mut params = self.params;
        if self.brush.has_connectivity() {
            params[1] = i32::from(!self.diagonals);
            params[2] = i32::from(!self.across);
        }
        Configure {
            hand: self.hand,
            settings: ws::Settings {
                brush: self.brush,
                params: Some(params),
                tool: self.tool,
                placement: self.placement,
                pattern: self.pattern,
            },
        }
    }
}

thread_local! {
    static PENDING: RefCell<Option<(ItemStack, Hand)>> = const { RefCell::new(None) };
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static LATER: RefCell<(Vec<Configure>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub(super) fn reset() {
    PENDING.with_borrow_mut(|p| *p = None);
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

/// `openHandgunGUI(item, hand)`.
pub(super) fn request(shaper: ItemStack, hand: Hand) {
    PENDING.with_borrow_mut(|p| *p = Some((shaper, hand)));
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if close {
        host.mod_screen_close(MOD);
    }
    for c in send {
        net::send_to_server(host, CHANNEL, &c.encode());
    }
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        net::send_to_server(host, CHANNEL, &open.configure().encode());
    }
    let Some((shaper, hand)) = PENDING.with_borrow_mut(Option::take) else {
        return;
    };
    OPEN.with_borrow_mut(|o| *o = Some(Open::new(shaper, hand)));
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

/// The window's buttons and inputs, from its corner `(x, y)`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Widget {
    Brush,
    Param(usize),
    Tool(usize),
    Diagonals,
    Across,
    Placement(usize),
    Pattern(usize),
    Confirm,
}

fn param_input(i: usize) -> [f32; 4] {
    [56.0 + 20.0 * i as f32, 40.0, 18.0, 18.0]
}

fn widgets(o: &Open) -> Vec<(Widget, [f32; 2])> {
    let mut w = vec![(Widget::Brush, [BRUSH_INPUT[0], BRUSH_INPUT[1]])];
    for i in 0..o.brush.params() {
        let r = param_input(i);
        w.push((Widget::Param(i), [r[0], r[1]]));
    }
    for i in 0..o.brush.tools().len() {
        w.push((Widget::Tool(i), [7.0 + 18.0 * i as f32, 79.0]));
    }
    if o.brush.has_connectivity() {
        w.push((Widget::Diagonals, [7.0 + 4.0 * 18.0, 79.0]));
        w.push((Widget::Across, [7.0 + 5.0 * 18.0, 79.0]));
    }
    if o.brush.has_placement() {
        for i in 0..PLACEMENTS.len() {
            w.push((Widget::Placement(i), [136.0 + 18.0 * i as f32, 79.0]));
        }
    }
    for i in 0..PATTERNS.len() {
        let (row, col) = (i / 3, i % 3);
        w.push((
            Widget::Pattern(i),
            [SIZE[0] - 76.0 + 18.0 * col as f32, 21.0 + 18.0 * row as f32],
        ));
    }
    w.push((Widget::Confirm, CONFIRM));
    w
}

fn rect(widget: Widget, at: [f32; 2], x: f32, y: f32) -> [f32; 4] {
    match widget {
        Widget::Brush => [x + at[0], y + at[1], BRUSH_INPUT[2], BRUSH_INPUT[3]],
        _ => square([x + at[0], y + at[1]]),
    }
}

fn hovered(o: &Open, x: f32, y: f32, mouse: [f32; 2]) -> Option<Widget> {
    widgets(o)
        .into_iter()
        .find(|(w, at)| inside(rect(*w, *at, x, y), mouse))
        .map(|(w, _)| w)
}

fn brush_names(host: &dyn ClientHost) -> Vec<String> {
    BRUSHES
        .iter()
        .map(|b| tr(host, &format!("gui.terrainzapper.brush.{}", b.name())))
        .collect()
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

/// `renderWindow` (`drawOnBackground`, `renderBlock`, `renderZapper`) and the widgets.
fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext, o: &Open, mouse: [f32; 2]) {
    super::gadget_gui::dim(host, draw);
    let (x, y) = corner(draw.size(), SIZE, OFFSET_X);
    draw.sprite(
        CURIOSITIES,
        [0.0, 0.0, SIZE[0], SIZE[1]],
        [x, y, SIZE[0], SIZE[1]],
        None,
    );
    let title = host.hover_name(&o.shaper);
    let w = draw.text_width(&title) as i32;
    draw.text(
        &title,
        [x + ((SIZE[0] as i32 - w) / 2) as f32, y + 4.0],
        TITLE,
        false,
    );
    // `TERRAINZAPPER_INACTIVE_PARAM` over the inputs the brush doesn't have.
    for i in o.brush.params()..3 {
        let r = param_input(i);
        draw.sprite(
            CURIOSITIES,
            [238.0, 0.0, 18.0, 18.0],
            [x + r[0], y + r[1], 18.0, 18.0],
            None,
        );
    }
    draw.text(
        &tr(host, "gui.terrainzapper.tool"),
        [x + 7.0, y + 69.0],
        FONT,
        false,
    );
    if o.brush.has_placement() {
        draw.text(
            &tr(host, "gui.terrainzapper.placement"),
            [x + 136.0, y + 69.0],
            FONT,
            false,
        );
    }
    // The brush's label, and each parameter's centred on its input.
    let brushes = brush_names(host);
    let b = BRUSHES.iter().position(|v| *v == o.brush).unwrap_or(0);
    draw.text(&brushes[b], [x + 61.0, y + 25.0], 0xFFFF_FFFF, true);
    for i in 0..o.brush.params() {
        let text = o.params[i].to_string();
        let tw = draw.text_width(&text) as i32;
        draw.text(
            &text,
            [x + (65 + 20 * i as i32 - tw / 2) as f32, y + 45.0],
            0xFFFF_FFFF,
            true,
        );
    }
    // The buttons.
    for (widget, at) in widgets(o) {
        let at = [x + at[0], y + at[1]];
        let (icon, green) = match widget {
            Widget::Tool(i) => {
                let t = o.brush.tools()[i];
                (tool_icon(t), t == o.tool)
            }
            Widget::Diagonals => {
                indicator(draw, [at[0], at[1] - 6.0], o.diagonals);
                (I_FOLLOW_DIAGONAL, false)
            }
            Widget::Across => {
                indicator(draw, [at[0], at[1] - 6.0], o.across);
                (I_FOLLOW_MATERIAL, false)
            }
            Widget::Placement(i) => (placement_icon(PLACEMENTS[i]), PLACEMENTS[i] == o.placement),
            Widget::Pattern(i) => (pattern_icon(PATTERNS[i]), PATTERNS[i] == o.pattern),
            Widget::Confirm => (I_CONFIRM, false),
            Widget::Brush | Widget::Param(_) => continue,
        };
        icon_button(draw, at, icon, green, mouse, o.pressed);
    }
    // `renderBlock`: the block it paints with, twenty pixels a block, turned 45° and tilted 25°.
    if let Some(state) = o.block.as_deref().and_then(|b| host.block_state_id(b)) {
        let _ = draw.draw_scene(
            [x + 14.0, y + 21.0, x + 50.0, y + 57.0],
            &mod_api::Scene::new(vec![SceneBlock::new([0, 0, 0], state, None)]),
            SceneView::new([0.5, 0.5, 0.5], 45.0, 25.0, 20.0),
        );
    }
    // `renderZapper`: the worldshaper four times its size, right of the window.
    draw.item_stack(&o.shaper, [x + SIZE[0], y + SIZE[1] - 48.0], 64.0);
    if let Some(widget) = hovered(o, x, y, mouse) {
        let lines = tooltip(host, o, widget, &brushes);
        if !lines.is_empty() {
            let _ = draw.gui_tooltip(&lines, mouse);
        }
    }
}

/// The hovered widget's tooltip: the inputs' (`ScrollInput.updateTooltip`) and the buttons'
/// one line.
fn tooltip(host: &dyn ClientHost, o: &Open, widget: Widget, brushes: &[String]) -> Vec<Text> {
    let line = |key: String| vec![Text::literal(tr(host, &key))];
    match widget {
        Widget::Brush => {
            let state = BRUSHES.iter().position(|b| *b == o.brush).unwrap_or(0);
            selection_tooltip(host, tr(host, "gui.terrainzapper.brush"), brushes, state)
        }
        Widget::Param(i) => scroll_tooltip(host, tr(host, o.brush.param_label(i))),
        Widget::Tool(i) => line(format!(
            "gui.terrainzapper.tool.{}",
            tool_key(o.brush.tools()[i])
        )),
        Widget::Diagonals => line("gui.terrainzapper.searchDiagonal".into()),
        Widget::Across => line("gui.terrainzapper.searchFuzzy".into()),
        Widget::Placement(i) => line(format!(
            "gui.terrainzapper.placement.{}",
            PLACEMENTS[i].name()
        )),
        Widget::Pattern(i) => line(format!(
            "gui.terrainzapper.pattern.{}",
            pattern_key(PATTERNS[i])
        )),
        Widget::Confirm => Vec::new(),
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
        match hovered(o, x, y, mouse) {
            Some(Widget::Brush) => {
                let state = BRUSHES.iter().position(|b| *b == o.brush).unwrap_or(0) as i32;
                if let Some(next) = gui::scroll(host, state, (0, 5), wheel, true, shift) {
                    o.brush = BRUSHES[next as usize];
                    o.brush_changed();
                }
            }
            Some(Widget::Param(i)) => {
                // `withRange(getMin, getMax + 1)`.
                let (lo, hi) = o.brush.bounds(i);
                if let Some(next) =
                    gui::scroll(host, o.params[i], (lo, hi + 1), wheel, false, shift)
                {
                    o.params[i] = next;
                }
            }
            _ => {}
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
    let close = OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return false;
        };
        o.pressed = event.button == 0;
        if event.button != 0 {
            return false;
        }
        let (x, y) = corner(o.size, SIZE, OFFSET_X);
        let Some(widget) = hovered(o, x, y, mouse) else {
            return false;
        };
        match widget {
            Widget::Tool(i) => o.tool = o.brush.tools()[i],
            Widget::Diagonals => o.diagonals = !o.diagonals,
            Widget::Across => o.across = !o.across,
            Widget::Placement(i) => o.placement = PLACEMENTS[i],
            Widget::Pattern(i) => o.pattern = PATTERNS[i],
            Widget::Confirm => {}
            Widget::Brush | Widget::Param(_) => return false,
        }
        gui::click_sound(host);
        widget == Widget::Confirm
    });
    if close && let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| {
            l.0.push(open.configure());
            l.1 = true;
        });
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

/// `removed`: the settings go to the server at the next tick.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| l.0.push(open.configure()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_cross_the_wire() {
        let c = Configure {
            hand: Hand::Main,
            settings: ws::Settings {
                brush: Brush::Cylinder,
                params: Some([8, 3, -1]),
                tool: Tool::Overlay,
                placement: Placement::Inserted,
                pattern: Pattern::Chance75,
            },
        };
        assert_eq!(Configure::decode(&c.encode()), Some(c));
        assert_eq!(Configure::decode(&[0; 16]), None);
    }
}
