//! The display link's screen (`DisplayLinkScreen`): the source read (from those of the block it
//! is attached to), its settings (a label, and the speed's direction, the stress's or power's
//! display, the time's format) and the line written. The server opens it with what the link
//! holds; the client sends the settings back when it closes (`DisplayLinkConfigurationPacket`),
//! right after the key or click (`Subsystem::input_handled`: the screen hooks are given a
//! read-only client).

use crate::glue::net;
use create_engine::display::{Config, Source};
use mod_api::{
    ClientHost, DrawContext, Flow, ScreenCharTypedPreEvent, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent,
    ScreenRenderPostEvent, SoundPlay,
};
use std::cell::RefCell;

/// Server to client: open the screen; client to server: the settings chosen.
pub(super) const CHANNEL: &str = "create:display_link_screen";
/// The mod whose own screen it is.
const MOD: &str = "create";

/// What the screen shows and sends back.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Settings {
    pub pos: [i32; 3],
    /// The sources of the block the link is attached to, and the one chosen.
    pub sources: Vec<Source>,
    pub source: usize,
    pub config: Config,
    /// The target: none (0), one line (1, a nixie tube's) or lines (2); its rows; the line.
    pub target: u8,
    pub rows: i32,
    pub line: i32,
    /// Whether the chosen source's label shows on this target (`allowsLabeling`).
    pub labels: Vec<bool>,
    /// The blocks read and written, for their icons.
    pub source_pos: [i32; 3],
    pub target_pos: [i32; 3],
    /// The name of the station read, for the station summary's filter (`populateData`).
    pub station: String,
}

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = &s.as_bytes()[..s.len().min(255)];
    out.push(b.len() as u8);
    out.extend_from_slice(b);
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }
    fn i32(&mut self) -> Option<i32> {
        self.bytes(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn str(&mut self) -> Option<String> {
        let n = usize::from(self.u8()?);
        String::from_utf8(self.bytes(n)?.to_vec()).ok()
    }
}

impl Settings {
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for v in self.pos {
            put_i32(&mut out, v);
        }
        out.push(self.sources.len() as u8);
        for (s, label) in self.sources.iter().zip(&self.labels) {
            out.push(Source::ALL.iter().position(|a| a == s).unwrap_or(0) as u8);
            out.push(u8::from(*label));
        }
        out.push(self.source as u8);
        put_str(&mut out, &self.config.label);
        put_i32(&mut out, self.config.directional);
        put_i32(&mut out, self.config.mode);
        put_i32(&mut out, self.config.cycle);
        put_i32(&mut out, self.config.format);
        put_i32(&mut out, self.config.interval);
        out.push(self.target);
        put_i32(&mut out, self.rows);
        put_i32(&mut out, self.line);
        for v in self.source_pos.into_iter().chain(self.target_pos) {
            put_i32(&mut out, v);
        }
        put_str(&mut out, &self.station);
        out.push(u8::from(self.config.filter.is_some()));
        put_str(&mut out, self.config.filter.as_deref().unwrap_or(""));
        put_i32(&mut out, self.config.name_column.unwrap_or(-1));
        put_i32(&mut out, self.config.platform_column.unwrap_or(-1));
        out
    }

    pub(super) fn decode(data: &[u8]) -> Option<Settings> {
        let mut r = Reader(data);
        let pos = [r.i32()?, r.i32()?, r.i32()?];
        let n = usize::from(r.u8()?);
        let mut sources = Vec::with_capacity(n);
        let mut labels = Vec::with_capacity(n);
        for _ in 0..n {
            sources.push(*Source::ALL.get(usize::from(r.u8()?))?);
            labels.push(r.u8()? != 0);
        }
        let source = usize::from(r.u8()?);
        let mut config = Config {
            label: r.str()?,
            directional: r.i32()?,
            mode: r.i32()?,
            cycle: r.i32()?,
            start_time: None,
            format: r.i32()?,
            interval: r.i32()?,
            ..Config::default()
        };
        let (target, rows, line) = (r.u8()?, r.i32()?, r.i32()?);
        let source_pos = [r.i32()?, r.i32()?, r.i32()?];
        let target_pos = [r.i32()?, r.i32()?, r.i32()?];
        let station = r.str()?;
        let has_filter = r.u8()? != 0;
        let filter = r.str()?;
        config.filter = has_filter.then_some(filter);
        config.name_column = Some(r.i32()?).filter(|v| *v >= 0);
        config.platform_column = Some(r.i32()?).filter(|v| *v >= 0);
        Some(Settings {
            pos,
            sources,
            source,
            config,
            target,
            rows,
            line,
            labels,
            source_pos,
            target_pos,
            station,
        })
    }
}

/// The second row's choice of a source (`initConfigurationWidgets`): its setting's key, the
/// lang keys of its options, and the input's width.
pub(super) fn choice(source: Source) -> Option<(&'static str, &'static [&'static str], f32)> {
    Some(match source {
        Source::KineticSpeed => (
            "Directional",
            &[
                "create.display_source.kinetic_speed.absolute",
                "create.display_source.kinetic_speed.directional",
            ],
            95.0,
        ),
        Source::KineticStress => (
            "Mode",
            &[
                "create.display_source.kinetic_stress.progress_bar",
                "create.display_source.kinetic_stress.percent",
                "create.display_source.kinetic_stress.current",
                "create.display_source.kinetic_stress.max",
                "create.display_source.kinetic_stress.remaining",
            ],
            120.0,
        ),
        Source::RedstonePower => (
            "Mode",
            &[
                "create.display_source.redstone_power.number",
                "create.display_source.redstone_power.progress_bar",
            ],
            120.0,
        ),
        Source::TimeOfDay => (
            "Cycle",
            &[
                "create.display_source.time.12_hour",
                "create.display_source.time.24_hour",
            ],
            60.0,
        ),
        Source::FillLevel => (
            "Mode",
            &[
                "create.display_source.fill_level.percent",
                "create.display_source.fill_level.progress_bar",
            ],
            120.0,
        ),
        Source::ItemThroughput => (
            "Interval",
            &[
                "create.display_source.item_throughput.interval.second",
                "create.display_source.item_throughput.interval.minute",
                "create.display_source.item_throughput.interval.hour",
            ],
            80.0,
        ),
        // ValueListDisplaySource.addFullNumberConfig, on the first row.
        Source::ListItems | Source::ListFluids => (
            "Format",
            &[
                "create.display_source.value_list.shortened",
                "create.display_source.value_list.full_number",
            ],
            75.0,
        ),
        _ => return None,
    })
}

/// Whether the source's choice sits on the first row (`isFirstLine`), where a label would.
fn choice_on_first_row(source: Source) -> bool {
    !source.single_line()
}

fn choice_value<'a>(config: &'a mut Config, key: &str) -> &'a mut i32 {
    match key {
        "Directional" => &mut config.directional,
        "Mode" => &mut config.mode,
        "Format" => &mut config.format,
        "Interval" => &mut config.interval,
        _ => &mut config.cycle,
    }
}

// ---------------------------------------------------------------------------------------------
// Client

struct Open {
    settings: Settings,
    /// Whether the label's text box has the focus.
    editing: bool,
    /// The screen's size when last drawn.
    size: [f32; 2],
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// Settings to send and whether to close, at the next client tick.
    static LATER: RefCell<(Vec<Settings>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub(super) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

/// The server opened the screen.
pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let Some(settings) = Settings::decode(data) else {
        return true;
    };
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            settings,
            editing: false,
            size: [0.0, 0.0],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    true
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if close {
        host.mod_screen_close(MOD);
    }
    for s in send {
        net::send_to_server(host, CHANNEL, &s.encode());
    }
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        net::send_to_server(host, CHANNEL, &open.settings.encode());
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn tr(host: &dyn ClientHost, key: &str) -> String {
    host.translate(key)
        .or_else(|| create_engine::display::en_us(key).map(str::to_owned))
        .unwrap_or_else(|| key.to_owned())
}

/// `AllGuiTextures.DATA_GATHERER`: the window's size.
const SIZE: [f32; 2] = [235.0, 162.0];

fn corner(size: [f32; 2]) -> (f32, f32) {
    (
        ((size[0] - SIZE[0]) / 2.0).floor(),
        ((size[1] - SIZE[1]) / 2.0).floor(),
    )
}

/// The screen's parts, as `[x, y, w, h]` from the window's corner.
const SOURCE_INPUT: [f32; 4] = [61.0, 26.0, 135.0, 16.0];
const LABEL_INPUT: [f32; 4] = [60.0, 47.0, 137.0, 18.0];
const LINE_INPUT: [f32; 4] = [61.0, 105.0, 135.0, 16.0];
const CONFIRM: [f32; 4] = [SIZE[0] - 33.0, SIZE[1] - 24.0, 18.0, 18.0];
/// The station summary's columns (`addScrollInput(0, 32, ..)` and `(36, 22, ..)` on the second
/// row).
const NAME_COLUMN: [f32; 4] = [60.0, 68.0, 32.0, 18.0];
const PLATFORM_COLUMN: [f32; 4] = [96.0, 68.0, 22.0, 18.0];

/// The text a source's first-row text box edits: the station summary's filter, else the label.
fn text_of(s: &mut Settings) -> &mut String {
    if s.sources.get(s.source) == Some(&Source::StationSummary) {
        return s.config.filter.get_or_insert_with(String::new);
    }
    &mut s.config.label
}

/// Whether the chosen source has a text box (`addTextInput`): a label, or the summary's filter.
fn has_text_box(s: &Settings) -> bool {
    s.labels.get(s.source).copied().unwrap_or(false)
        || s.sources.get(s.source) == Some(&Source::StationSummary)
}

/// A scroll input: its rect, value, range end (exclusive), shift step, suffix and title key.
type ScrollInput = ([f32; 4], i32, i32, i32, &'static str, &'static str);

/// The station summary's scroll inputs.
fn summary_inputs(c: &Config) -> [ScrollInput; 2] {
    [
        (
            NAME_COLUMN,
            c.name_column.unwrap_or(50),
            73,
            12,
            "%",
            "create.display_source.station_summary.train_name_column",
        ),
        (
            PLATFORM_COLUMN,
            c.platform_column.unwrap_or(3),
            16,
            4,
            "",
            "create.display_source.station_summary.platform_column",
        ),
    ]
}

fn styled(text: String, color: &str, italic: bool) -> mod_api::Text {
    let mut style = mod_api::text::TextStyle::default();
    style.color = Some(color.to_owned());
    style.italic = italic.then_some(true);
    mod_api::Text::literal(text).with_style(style)
}

fn choice_input(width: f32, first_row: bool) -> [f32; 4] {
    [60.0, if first_row { 47.0 } else { 68.0 }, width, 18.0]
}

fn at(rect: [f32; 4], left: f32, top: f32) -> [f32; 4] {
    [left + rect[0], top + rect[1], rect[2], rect[3]]
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

fn click_sound(host: &dyn ClientHost) {
    host.play_local_sound(
        &SoundPlay::ui("minecraft:ui.button.click").with_volume(0.25),
        0,
    );
}

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let Some(open) = OPEN.with_borrow_mut(|o| {
        o.as_mut().map(|o| {
            o.size = size;
            (o.settings.clone(), o.editing)
        })
    }) else {
        return;
    };
    draw(host, event.draw, &open.0, open.1, event.screen.mouse);
}

/// `DisplayLinkScreen.renderWindow` and its widgets.
fn draw(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    s: &Settings,
    editing: bool,
    mouse: [f32; 2],
) {
    let (x, y) = corner(draw.size());
    draw.sprite(
        "create:gui/display_link",
        [0.0, 0.0, SIZE[0], SIZE[1]],
        [x, y, SIZE[0], SIZE[1]],
        None,
    );
    let title = tr(host, "create.display_link.title");
    let w = draw.text_width(&title);
    draw.text(
        &title,
        [x + SIZE[0] / 2.0 - w / 2.0, y + 4.0],
        0xFF59_2424,
        false,
    );
    let source = s.sources.get(s.source).copied();
    match source {
        None => draw.text(
            &tr(host, "create.display_link.no_source"),
            [x + 65.0, y + 30.0],
            0xFFD3_D3D3,
            true,
        ),
        Some(source) => {
            draw.text(
                &tr(host, source.name_key()),
                [x + 65.0, y + 30.0],
                0xFFFF_FFFF,
                true,
            );
            if s.sources.len() > 1 && inside(at(SOURCE_INPUT, x, y), mouse) {
                let tip = tr(host, "create.display_link.information_type");
                draw.text(&tip, [mouse[0] + 8.0, mouse[1] - 12.0], 0xFFFF_FFFF, true);
            }
            // The station summary: its filter's text box, and its columns' scroll inputs.
            if source == Source::StationSummary {
                let r = at(LABEL_INPUT, x, y);
                draw.fill(r, 0xFF39_3939);
                draw.fill(
                    [r[0] + 1.0, r[1] + 1.0, r[2] - 2.0, r[3] - 2.0],
                    0xFF0F_0F0F,
                );
                let mut text = s.config.filter.clone().unwrap_or_default();
                if editing && (host_millis() / 300).is_multiple_of(2) {
                    text.push('_');
                }
                draw.text(&text, [r[0] + 5.0, r[1] + 5.0], 0xFFFF_FFFF, true);
                let mut tips = Vec::new();
                if inside(r, mouse) {
                    tips = vec![
                        styled(
                            tr(host, "create.display_source.station_summary.filter"),
                            "#5391E1",
                            false,
                        ),
                        styled(tr(host, "create.gui.schedule.lmb_edit"), "dark_gray", true),
                    ];
                }
                for (rect, value, _, _, suffix, title) in summary_inputs(&s.config) {
                    let r = at(rect, x, y);
                    draw.fill(r, 0xFF39_3939);
                    draw.fill(
                        [r[0] + 1.0, r[1] + 1.0, r[2] - 2.0, r[3] - 2.0],
                        0xFF5A_4A3A,
                    );
                    draw.text(
                        &format!("{value}{suffix}"),
                        [r[0] + 5.0, r[1] + 5.0],
                        0xFFFF_FFFF,
                        true,
                    );
                    if inside(r, mouse) {
                        // `ScrollInput.updateTooltip`.
                        tips = vec![
                            styled(tr(host, title), "#5391E1", false),
                            styled(
                                tr(host, "create.gui.scrollInput.scrollToModify"),
                                "dark_gray",
                                true,
                            ),
                            styled(
                                tr(host, "create.gui.scrollInput.shiftScrollsFaster"),
                                "dark_gray",
                                true,
                            ),
                        ];
                    }
                }
                if !tips.is_empty() {
                    let _ = draw.gui_tooltip(&tips, mouse);
                }
            }
            // The first row: the label (`addLabelingTextBox`).
            if s.labels.get(s.source).copied().unwrap_or(false) {
                let r = at(LABEL_INPUT, x, y);
                draw.fill(r, 0xFF39_3939);
                draw.fill(
                    [r[0] + 1.0, r[1] + 1.0, r[2] - 2.0, r[3] - 2.0],
                    0xFF0F_0F0F,
                );
                let mut text = s.config.label.clone();
                if text.is_empty() && !editing {
                    draw.text(
                        &tr(host, "create.display_source.label"),
                        [r[0] + 4.0, r[1] + 5.0],
                        0xFF70_7070,
                        false,
                    );
                } else {
                    if editing && (host_millis() / 300).is_multiple_of(2) {
                        text.push('_');
                    }
                    draw.text(&text, [r[0] + 4.0, r[1] + 5.0], 0xFFFF_FFFF, true);
                }
            }
            // The second row: the source's choice.
            if let Some((key, options, width)) = choice(source) {
                let r = at(choice_input(width, choice_on_first_row(source)), x, y);
                draw.fill(r, 0xFF39_3939);
                draw.fill(
                    [r[0] + 1.0, r[1] + 1.0, r[2] - 2.0, r[3] - 2.0],
                    0xFF5A_4A3A,
                );
                let mut c = s.config.clone();
                let v = *choice_value(&mut c, key);
                let i = (v.max(0) as usize).min(options.len() - 1);
                draw.text(
                    &tr(host, options[i]),
                    [r[0] + 4.0, r[1] + 5.0],
                    0xFFFF_FFFF,
                    true,
                );
            }
        }
    }
    // The line written (`getLineOptionText`).
    match s.target {
        0 => draw.text(
            &tr(host, "create.display_link.no_target"),
            [x + 65.0, y + 109.0],
            0xFFD3_D3D3,
            true,
        ),
        1 => draw.text(
            &tr(host, "create.display_target.single_line"),
            [x + 65.0, y + 109.0],
            0xFFFF_FFFF,
            true,
        ),
        // LecternDisplayTarget.getLineOptionText.
        3 => {
            let page = tr(host, "create.display_target.page")
                .replace("%s", &(s.line + 1).to_string())
                .replace("%1$s", &(s.line + 1).to_string());
            draw.text(&page, [x + 65.0, y + 109.0], 0xFFFF_FFFF, true);
        }
        _ => {
            let line = tr(host, "create.display_target.line")
                .replace("%s", &(s.line + 1).to_string())
                .replace("%1$s", &(s.line + 1).to_string());
            draw.text(&line, [x + 65.0, y + 109.0], 0xFFFF_FFFF, true);
        }
    }
    // The blocks read and written (`ElementWidget`s of their items, a barrier for none).
    for (p, row) in [(s.source_pos, 26.0), (s.target_pos, 105.0)] {
        let item = block_item(host, p).or_else(|| item_key(host, "minecraft:barrier"));
        if let Some(item) = item {
            draw.item(item, [x + 37.0, y + row], 16.0);
        }
    }
    // The confirm button (`IconButton`, `AllIcons.I_CONFIRM`).
    let b = at(CONFIRM, x, y);
    let hovered = inside(b, mouse);
    draw.sprite(
        "create:gui/widgets",
        [if hovered { 18.0 } else { 0.0 }, 0.0, 18.0, 18.0],
        b,
        None,
    );
    draw.sprite(
        "create:gui/icons",
        [0.0, 16.0, 16.0, 16.0],
        [b[0] + 1.0, b[1] + 1.0, 16.0, 16.0],
        None,
    );
}

fn item_key(host: &dyn ClientHost, id: &str) -> Option<mod_api::ItemKey> {
    mod_api::Identifier::parse(id)
        .ok()
        .and_then(|i| host.item_key(&i))
}

/// The item of the block at `pos` this client shows (`getCloneItemStack`, by name): a nixie
/// tube of any colour is the orange one's.
fn block_item(host: &dyn ClientHost, pos: [i32; 3]) -> Option<mod_api::ItemKey> {
    let state = host.level(host.dimension())?.block_state(host, pos)?;
    let text = host.block_state_string(state)?;
    let name = text.split('[').next()?;
    if name == "minecraft:air" {
        return None;
    }
    let name = if name.ends_with("nixie_tube") {
        "create:nixie_tube"
    } else {
        name
    };
    item_key(host, name)
}

/// Milliseconds for the text cursor's blink.
fn host_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// A step of the input under `mouse` (`ScrollInput`): the source, the source's choice or the
/// line. True when one took it.
fn step(s: &mut Settings, mouse: [f32; 2], size: [f32; 2], up: bool, shift: bool) -> bool {
    let (x, y) = corner(size);
    if s.sources.get(s.source) == Some(&Source::StationSummary) {
        // `ScrollInput.mouseScrolled`: up adds, shift by its shift step (then to a multiple).
        for (i, (rect, value, max, shift_step, _, _)) in
            summary_inputs(&s.config).into_iter().enumerate()
        {
            if !inside(at(rect, x, y), mouse) {
                continue;
            }
            let delta = if shift { shift_step } else { 1 };
            let mut v = value + if up { delta } else { -delta };
            if shift {
                v -= v % shift_step;
            }
            let v = v.clamp(0, max - 1);
            if v == value {
                return false;
            }
            if i == 0 {
                s.config.name_column = Some(v);
            } else {
                s.config.platform_column = Some(v);
            }
            return true;
        }
    }
    if s.sources.len() > 1 && inside(at(SOURCE_INPUT, x, y), mouse) {
        let n = s.sources.len();
        s.source = if up {
            (s.source + n - 1) % n
        } else {
            (s.source + 1) % n
        };
        // `DisplayLinkConfigurationPacket`: another source starts from its own settings
        // (`populateData` fills the station summary's).
        s.config = Config::default();
        if s.sources.get(s.source) == Some(&Source::StationSummary) {
            let station = (!s.station.is_empty()).then_some(s.station.as_str());
            super::summary::populate(&mut s.config, station);
        }
        return true;
    }
    if let Some(source) = s.sources.get(s.source).copied()
        && let Some((key, options, width)) = choice(source)
        && inside(
            at(choice_input(width, choice_on_first_row(source)), x, y),
            mouse,
        )
    {
        let v = choice_value(&mut s.config, key);
        let max = options.len() as i32 - 1;
        *v = (if up { *v - 1 } else { *v + 1 }).clamp(0, max);
        return true;
    }
    if s.target >= 2 && s.rows > 1 && inside(at(LINE_INPUT, x, y), mouse) {
        // `inverted()`: up is the line before.
        s.line = (if up { s.line - 1 } else { s.line + 1 }).clamp(0, s.rows - 1);
        return true;
    }
    false
}

pub(super) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.scroll[1] == 0.0 {
        return Flow::Continue;
    }
    let up = event.scroll[1] > 0.0;
    let mouse = event.screen.mouse;
    // GLFW's shift modifier.
    let shift = event.modifiers & 1 != 0;
    let stepped = OPEN.with_borrow_mut(|o| {
        o.as_mut()
            .is_some_and(|o| step(&mut o.settings, mouse, o.size, up, shift))
    });
    if stepped {
        click_sound(host);
    }
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
    let size = OPEN.with_borrow(|o| o.as_ref().map_or([0.0, 0.0], |o| o.size));
    let (x, y) = corner(size);
    if event.button == 0 && inside(at(CONFIRM, x, y), mouse) {
        click_sound(host);
        if let Some(open) = OPEN.with_borrow_mut(Option::take) {
            LATER.with_borrow_mut(|l| {
                l.0.push(open.settings);
                l.1 = true;
            });
        }
        return Flow::Cancel;
    }
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        o.editing = has_text_box(&o.settings) && inside(at(LABEL_INPUT, x, y), mouse);
        if event.button == 0 && !o.editing {
            // A click steps on like the wheel down.
            if step(&mut o.settings, mouse, size, false, false) {
                click_sound(host);
            }
        }
    });
    Flow::Cancel
}

pub(super) fn screen_key_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenKeyPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let editing = OPEN.with_borrow(|o| o.as_ref().is_some_and(|o| o.editing));
    if !editing {
        return Flow::Continue;
    }
    match event.key {
        "key.keyboard.backspace" => {
            OPEN.with_borrow_mut(|o| {
                if let Some(o) = o.as_mut() {
                    text_of(&mut o.settings).pop();
                }
            });
            Flow::Cancel
        }
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => {
            OPEN.with_borrow_mut(|o| {
                if let Some(o) = o.as_mut() {
                    o.editing = false;
                }
            });
            Flow::Cancel
        }
        "key.keyboard.escape" => Flow::Continue,
        // Typed letters arrive as characters; keep the game's own keys (inventory) out.
        _ => Flow::Cancel,
    }
}

pub(super) fn screen_char_typed(
    host: &dyn ClientHost,
    event: &mut ScreenCharTypedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let c = event.c;
    if c == '\u{a7}' || c < ' ' || c == '\u{7f}' {
        return Flow::Cancel;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut()
            && o.editing
        {
            // `EditBox`'s 32 characters for the filter.
            let summary =
                o.settings.sources.get(o.settings.source) == Some(&Source::StationSummary);
            let text = text_of(&mut o.settings);
            if text.chars().count() < if summary { 32 } else { 64 } {
                text.push(c);
            }
        }
    });
    Flow::Cancel
}

/// `onClose`: the settings go to the server at the next tick.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| l.0.push(open.settings));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_cross_the_wire() {
        let s = Settings {
            pos: [1, -2, 3],
            sources: vec![Source::TimeOfDay, Source::StopWatch],
            source: 1,
            config: Config {
                label: "Clock".into(),
                cycle: 1,
                ..Config::default()
            },
            target: 2,
            rows: 4,
            line: 2,
            labels: vec![true, false],
            source_pos: [1, -2, 2],
            target_pos: [9, 9, 9],
            station: "Main".into(),
        };
        assert_eq!(Settings::decode(&s.encode()), Some(s));
    }
}
