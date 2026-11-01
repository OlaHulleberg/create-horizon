//! Create 6's value settings board: hold use on a value box and a board of rows opens
//! (`ValueSettingsClient.tick`), the cursor picks a row and a column (`ValueSettingsScreen`),
//! releasing use sends the pick (`ValueSettingsPacket`) and the block's behaviour turns it into
//! its value (`ValueSettingsBehaviour.setValueSettings`). Layout numbers are GUI pixels; text
//! widths come from the game's font.

/// `ValueSettingsBehaviour.ValueSettings`: a row of the board and a column on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub row: i32,
    pub value: i32,
}

/// `ValueSettingsBoard`: a title, columns 0 to `max`, a milestone every `milestone` columns,
/// one row per label; `icons` for a `ScrollOptionSettingsFormatter` (options drawn as icons,
/// no row labels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    pub title: &'static str,
    pub max: i32,
    pub milestone: i32,
    pub rows: &'static [&'static str],
    pub icons: bool,
}

/// `ScrollValueBehaviour.createBoard`: one row, a milestone every 10.
pub const fn scroll_value(title: &'static str, max: i32) -> Board {
    Board {
        title,
        max,
        milestone: 10,
        rows: &["Value"],
        icons: false,
    }
}

/// `KineticScrollValueBehaviour.createBoard` (the creative motor, the speed controller):
/// clockwise and counter-clockwise rows, 256 columns, a milestone every 32.
pub const fn kinetic(title: &'static str) -> Board {
    Board {
        title,
        max: 256,
        milestone: 32,
        rows: &["\u{27f3}", "\u{27f2}"],
        icons: false,
    }
}

/// The creative motor's board (`CreativeMotorBlockEntity`,
/// `create.kinetics.creative_motor.rotation_speed`).
pub const MOTOR: Board = kinetic("Generated Speed in RPM");

/// `ChassisScrollValueBehaviour.createBoard`: one row, columns for ranges 1 to `max_range`
/// (`maxChassisRange`, 16 by default), each its own milestone.
pub const fn chassis(title: &'static str, max_range: i32) -> Board {
    Board {
        title,
        max: max_range - 1,
        milestone: 1,
        rows: &["Distance"],
        icons: false,
    }
}

/// `ScrollOptionBehaviour.createBoard`: one "Select" row of `options` icons.
pub const fn options(title: &'static str, options: i32) -> Board {
    Board {
        title,
        max: options - 1,
        milestone: 1,
        rows: &["Select"],
        icons: true,
    }
}

/// `BrassDiodeScrollValueBehaviour.createBoard`: ticks, seconds and minutes up to 60.
pub const fn brass_diode(title: &'static str) -> Board {
    Board {
        title,
        max: 60,
        milestone: 10,
        rows: &["Ticks", "Seconds", "Minutes"],
        icons: false,
    }
}

/// `ValveHandleScrollValueBehaviour.createBoard`: both directions up to 180°, a milestone every 45.
pub const VALVE_HANDLE: Board = Board {
    title: "Rotation when used",
    max: 180,
    milestone: 45,
    rows: &["\u{27f3}", "\u{27f2}"],
    icons: false,
};

/// `create.gui.value_settings.hold_to_edit`: the hover tip's second line.
pub const HOLD_TO_EDIT: &str = "Click and hold to edit";

/// `create.gui.value_settings.release_to_confirm` with the use key's name.
pub fn release_to_confirm(key: &str) -> String {
    format!("Release {key} to Confirm")
}

impl Board {
    /// Pixels per column: 1 past 128 columns, else 2.
    pub fn scale(&self) -> i32 {
        if self.max > 128 { 1 } else { 2 }
    }

    /// A milestone's width: 8 for icons, else 4.
    pub fn milestone_size(&self) -> i32 {
        if self.icons { 8 } else { 4 }
    }

    pub fn milestone_count(&self) -> i32 {
        self.max / self.milestone + 1
    }

    /// `valueBarWidth`.
    pub fn bar_width(&self) -> i32 {
        (self.max + 1) * self.scale() + 1 + self.milestone_count() * self.milestone_size()
    }

    /// The column the wheel moves to from `closest` (`mouseScrolled`): one per step, a milestone
    /// with shift, within the board; None when it stays.
    pub fn scrolled(&self, closest: Settings, up: bool, shift: bool) -> Option<i32> {
        let step = if shift { self.milestone } else { 1 };
        let column = (closest.value + if up { step } else { -step }).clamp(0, self.max);
        (column != closest.value).then_some(column)
    }

    /// `ValueSettingsScreen.renderWindow`: the pitch of the tick heard when the cursor moves
    /// to `value` (`Mth.lerp(value / max, 1.15, 1.5)`).
    pub fn pitch(&self, value: i32) -> f32 {
        let t = value as f32 / self.max as f32;
        1.15 + t * (1.5 - 1.15)
    }
}

/// The board laid out on a screen (`ValueSettingsScreen.init`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub board: Board,
    /// `guiLeft`, `guiTop`.
    pub left: i32,
    pub top: i32,
    /// `maxLabelWidth`: the widest row label, -18 for icons.
    pub label: i32,
    pub width: i32,
    pub height: i32,
}

impl Layout {
    /// The window for `board` with its widest row label `label_width` wide, centred on a
    /// `screen` GUI pixels big (catnip's `AbstractSimiScreen.init`, which is not in this
    /// repository: centring is assumed).
    pub fn new(board: Board, label_width: i32, screen: [i32; 2]) -> Layout {
        let label = if board.icons { -18 } else { label_width };
        let width = (label + 14) + (board.bar_width() + 10);
        let height = board.rows.len() as i32 * 11;
        Layout {
            board,
            left: (screen[0] - width) / 2,
            top: (screen[1] - height) / 2,
            label,
            width,
            height,
        }
    }

    /// The left of the value bars (`valueBarX`).
    pub fn bar_x(&self) -> i32 {
        self.left + self.label + 14 + 4
    }

    /// `getCoordinateOfValue`: the centre of `column` on `row`.
    pub fn coordinate(&self, row: i32, column: i32) -> [f32; 2] {
        let b = &self.board;
        let size = b.milestone_size();
        let mut x = (self.left + ((column.max(1) - 1) / b.milestone) * size + column * b.scale())
            as f32
            + 1.5;
        x += (self.label + 14 + 4) as f32;
        if column % b.milestone == 0 {
            x += (size / 2) as f32;
        }
        if column > 0 {
            x += size as f32;
        }
        let y = self.top as f32 + (row as f32 + 0.5) * 11.0 - 0.5;
        [x, y]
    }

    /// `getClosestCoordinate`: the row and column under the mouse; with shift only milestones.
    pub fn closest(&self, mouse: [i32; 2], milestones_only: bool) -> Settings {
        let b = &self.board;
        let mut row = 0;
        let mut best = f64::MAX;
        while row < b.rows.len() as i32 {
            let diff = (f64::from(self.coordinate(row, 0)[1]) - f64::from(mouse[1])).abs();
            if best < diff {
                break;
            }
            best = diff;
            row += 1;
        }
        row -= 1;
        let mut column = 0;
        best = f64::MAX;
        while column <= b.max {
            let c = if milestones_only {
                column * b.milestone
            } else {
                column
            };
            let diff = (f64::from(self.coordinate(row, c)[0]) - f64::from(mouse[0])).abs();
            if best < diff {
                break;
            }
            best = diff;
            column += 1;
        }
        column -= 1;
        let value = if milestones_only {
            (column * b.milestone).min(b.max)
        } else {
            column
        };
        Settings { row, value }
    }

    /// The left of each milestone mark on a row.
    pub fn milestones(&self) -> impl Iterator<Item = i32> + '_ {
        let b = &self.board;
        let step = b.milestone_size() + b.milestone * b.scale();
        (0..b.milestone_count()).map(move |i| self.bar_x() + i * step)
    }

    /// The cursor's box for a text `text_width` wide at `coordinate` (16 for an icon):
    /// `[x, y, w]`, 14 high.
    pub fn cursor(coordinate: [f32; 2], text_width: i32) -> [i32; 3] {
        let w = (text_width / 2) * 2 + 3;
        [coordinate[0] as i32 - w / 2, coordinate[1] as i32 - 7, w]
    }

    /// The background behind the board after `open` ticks (with the partial tick), with the
    /// widest of the title, the tip and (for icons) the options `fattest` wide.
    pub fn frame(&self, fattest: i32, open: f32) -> Frame {
        let fade = (open / 4.0).clamp(0.0, 1.0);
        let fat_tip = 0.max(fattest + 10 - (self.width + 13)) / 2;
        let bg_width = (self.width + 13).max(fattest + 10);
        let width = (bg_width as f32 * fade) as i32;
        let start = (bg_width - width) / 2 - fat_tip;
        let extra = if self.board.icons { 46 } else { 33 };
        Frame {
            x: self.left - 11 + start,
            y: self.top - 17,
            width,
            height: self.height + extra,
            text: width > fattest,
            text_x: self.left - 11 - fat_tip + bg_width / 2,
            title_y: self.top - 14,
            tip_y: self.top + self.height + extra - 27,
            option_y: self.top + self.height + extra - 40,
        }
    }
}

/// What `renderWindow` draws around the bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The background, growing from the middle as the board fades in.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Whether the title and tip show yet, centred on `text_x`.
    pub text: bool,
    pub text_x: i32,
    pub title_y: i32,
    pub tip_y: i32,
    /// An icon board's option name, under the bars.
    pub option_y: i32,
}

/// The screen's darkening (`renderBackground`): ARGB, fading in over 20 ticks.
pub fn shade(open: f32) -> u32 {
    let a = (0x50 as f32 * (open / 20.0).min(1.0)) as u32;
    (a << 24) | 0x10_1010
}

/// The press that opens a board (`ValueSettingsClient.interactHeldTicks`): counted per client
/// tick from the click on the value box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hold {
    ticks: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    /// Use was let go first: a short click (`onShortInteract`).
    Short,
    Open,
}

impl Hold {
    /// `ValueSettingsClient.tick` with use `down`: the board opens on the sixth tick held.
    pub fn tick(&mut self, down: bool) -> Step {
        if !down {
            return Step::Short;
        }
        let t = self.ticks;
        self.ticks += 1;
        if t < 5 { Step::Wait } else { Step::Open }
    }
}

/// The tip over the hotbar while a value box is looked at (`ValueSettingsClient.showHoverTip`
/// each tick it is, `tick` every tick).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HoverTip {
    pub ticks: i32,
    pub warmup: i32,
}

impl HoverTip {
    pub fn tick(&mut self) {
        if self.warmup > 0 {
            self.warmup -= 1;
        }
        if self.ticks > 0 {
            self.ticks -= 1;
        }
    }

    /// The box is looked at this tick: after a short warm-up the tip shows.
    pub fn show(&mut self) {
        if self.warmup < 6 {
            self.warmup += 2;
            return;
        }
        self.warmup += 1;
        self.ticks = if self.ticks == 0 {
            11
        } else {
            self.ticks.max(6)
        };
    }

    /// Its opacity, None while hidden (`ValueSettingsClient.render`).
    pub fn alpha(&self) -> Option<f32> {
        if self.ticks == 0 {
            return None;
        }
        Some(if self.ticks > 5 {
            (11 - self.ticks) as f32 / 5.0
        } else {
            (self.ticks as f32 / 5.0).min(1.0)
        })
    }
}

/// `CreateLang.number` for whole numbers (`LangNumberFormat`, en_us): thousands grouped.
pub fn number(v: i32) -> String {
    let digits = v.unsigned_abs().to_string();
    let mut out = String::new();
    if v < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `KineticScrollValueBehaviour.getValueSettings`: a speed's row (0 counter to the axis) and size.
pub fn kinetic_settings(value: i32) -> Settings {
    Settings {
        row: if value < 0 { 0 } else { 1 },
        value: value.abs(),
    }
}

/// `KineticScrollValueBehaviour.setValueSettings` then `ScrollValueBehaviour.setValue` within
/// `-limit..=limit`: never 0.
pub fn kinetic_value(s: Settings, limit: i32) -> i32 {
    let v = s.value.max(1);
    (if s.row == 0 { -v } else { v }).clamp(-limit, limit)
}

/// `KineticScrollValueBehaviour.formatSettings`.
pub fn kinetic_text(s: Settings) -> String {
    let arrow = if s.row == 0 { "\u{27f3}" } else { "\u{27f2}" };
    format!("{}{arrow}", number(s.value.abs().max(1)))
}

/// `ChassisScrollValueBehaviour.getValueSettings`: column 0 is range 1.
pub fn chassis_settings(range: i32) -> Settings {
    Settings {
        row: 0,
        value: range - 1,
    }
}

/// `ChassisScrollValueBehaviour.setValueSettings`, within `between(1, max_range)`.
pub fn chassis_value(s: Settings, max_range: i32) -> i32 {
    (s.value + 1).clamp(1, max_range)
}

/// The chassis board's formatter.
pub fn chassis_text(s: Settings) -> String {
    number(s.value + 1)
}

/// `ScrollOptionBehaviour`: the option picked, within `between(0, options - 1)`.
pub fn option_value(s: Settings, options: i32) -> i32 {
    s.value.clamp(0, options - 1)
}

/// `BrassDiodeScrollValueBehaviour.getValueSettings`: a delay in ticks as ticks (up to 60),
/// seconds (up to a minute) or minutes.
pub fn diode_settings(ticks: i32) -> Settings {
    if ticks > 60 * 20 {
        Settings {
            row: 2,
            value: ticks / (60 * 20),
        }
    } else if ticks > 60 {
        Settings {
            row: 1,
            value: ticks / 20,
        }
    } else {
        Settings {
            row: 0,
            value: ticks,
        }
    }
}

/// `BrassDiodeScrollValueBehaviour.setValueSettings`, within `between(2, 60 * 20 * 60)`.
pub fn diode_value(s: Settings) -> i32 {
    let multiplier = match s.row {
        0 => 1,
        1 => 20,
        _ => 60 * 20,
    };
    (s.value.max(1) * multiplier).max(2).clamp(2, 60 * 20 * 60)
}

/// `BrassDiodeScrollValueBehaviour.formatSettings`.
pub fn diode_text(s: Settings) -> String {
    let v = s.value.max(1);
    match s.row {
        0 => format!("{}t", v.max(2)),
        1 => format!("0:{}{v}", if v < 10 { "0" } else { "" }),
        _ => format!("{v}:00"),
    }
}

/// `ValveHandleScrollValueBehaviour.formatValue`.
pub fn valve_text(s: Settings) -> String {
    format!("{}\u{b0}", number(s.value.abs().max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motor(label: i32) -> Layout {
        Layout {
            left: 0,
            top: 0,
            ..Layout::new(MOTOR, label, [0, 0])
        }
    }

    #[test]
    fn board_sizes_follow_the_screen() {
        // 256 columns at 1 px, 9 milestones of 4: 257 + 1 + 36.
        assert_eq!(MOTOR.scale(), 1);
        assert_eq!(MOTOR.milestone_count(), 9);
        assert_eq!(MOTOR.bar_width(), 294);
        let l = Layout::new(MOTOR, 8, [427, 240]);
        assert_eq!((l.width, l.height), (8 + 14 + 294 + 10, 22));
        assert_eq!((l.left, l.top), ((427 - 326) / 2, (240 - 22) / 2));
        // Chassis: 16 ranges at 2 px, each a milestone.
        let c = chassis("Range of Sticky Sides", 16);
        assert_eq!((c.max, c.scale(), c.milestone_count()), (15, 2, 16));
        assert_eq!(c.bar_width(), 16 * 2 + 1 + 16 * 4);
        // Icons: label -18 and 8 px milestones.
        let o = options("Distribution Method", 7);
        assert_eq!(o.bar_width(), 7 * 2 + 1 + 7 * 8);
        let l = Layout::new(o, 99, [0, 0]);
        assert_eq!((l.label, l.width, l.height), (-18, -4 + 71 + 10, 11));
    }

    #[test]
    fn columns_sit_between_milestones() {
        let l = motor(10);
        // Column 0 is past the first milestone's middle; 1 past the whole milestone.
        assert_eq!(l.coordinate(0, 0), [10.0 + 21.5, 5.0]);
        assert_eq!(l.coordinate(0, 1), [10.0 + 24.5, 5.0]);
        assert_eq!(l.coordinate(0, 32), [10.0 + 57.5, 5.0]);
        assert_eq!(l.coordinate(0, 33), [10.0 + 60.5, 5.0]);
        assert_eq!(l.coordinate(1, 256), [10.0 + 309.5, 16.0]);
        let marks: Vec<i32> = l.milestones().take(3).collect();
        assert_eq!(marks, [28, 64, 100]);
    }

    #[test]
    fn the_cursor_picks_the_nearest_row_and_column() {
        let l = motor(10);
        assert_eq!(l.closest([67, 10], false), Settings { row: 0, value: 32 });
        assert_eq!(l.closest([67, 11], false), Settings { row: 1, value: 32 });
        // Past either end it stays on the board.
        assert_eq!(l.closest([-50, -50], false), Settings { row: 0, value: 0 });
        assert_eq!(
            l.closest([900, 900], false),
            Settings { row: 1, value: 256 }
        );
        // With shift only milestones.
        assert_eq!(l.closest([70, 5], true), Settings { row: 0, value: 32 });
        assert_eq!(l.closest([900, 5], true), Settings { row: 0, value: 256 });
        // Every column is its own nearest.
        for v in 0..=256 {
            let [x, y] = l.coordinate(1, v);
            assert_eq!(
                l.closest([x as i32, y as i32], false),
                Settings { row: 1, value: v }
            );
        }
    }

    #[test]
    fn the_wheel_moves_a_column_or_a_milestone() {
        let s = Settings { row: 1, value: 255 };
        assert_eq!(MOTOR.scrolled(s, true, false), Some(256));
        assert_eq!(MOTOR.scrolled(s, true, true), Some(256));
        assert_eq!(
            MOTOR.scrolled(Settings { row: 1, value: 256 }, true, false),
            None
        );
        assert_eq!(MOTOR.scrolled(s, false, true), Some(223));
        assert_eq!(MOTOR.scrolled(Settings::default(), false, false), None);
    }

    #[test]
    fn values_round_trip_through_settings() {
        assert_eq!(kinetic_settings(-16), Settings { row: 0, value: 16 });
        assert_eq!(kinetic_settings(16), Settings { row: 1, value: 16 });
        assert_eq!(kinetic_value(Settings { row: 0, value: 0 }, 256), -1);
        assert_eq!(kinetic_value(Settings { row: 1, value: 300 }, 256), 256);
        assert_eq!(kinetic_text(Settings { row: 0, value: 0 }), "1\u{27f3}");
        assert_eq!(chassis_settings(16), Settings { row: 0, value: 15 });
        assert_eq!(chassis_value(Settings { row: 0, value: 15 }, 16), 16);
        assert_eq!(chassis_text(Settings { row: 0, value: 0 }), "1");
        assert_eq!(option_value(Settings { row: 0, value: 9 }, 5), 4);
        assert_eq!(diode_settings(40), Settings { row: 0, value: 40 });
        assert_eq!(diode_settings(100), Settings { row: 1, value: 5 });
        assert_eq!(diode_settings(2400), Settings { row: 2, value: 2 });
        assert_eq!(diode_value(Settings { row: 0, value: 0 }), 2);
        assert_eq!(diode_value(Settings { row: 1, value: 5 }), 100);
        assert_eq!(diode_value(Settings { row: 2, value: 60 }), 72000);
        assert_eq!(diode_text(Settings { row: 0, value: 1 }), "2t");
        assert_eq!(diode_text(Settings { row: 1, value: 5 }), "0:05");
        assert_eq!(diode_text(Settings { row: 2, value: 3 }), "3:00");
        assert_eq!(valve_text(Settings { row: 1, value: 90 }), "90\u{b0}");
        assert_eq!(number(-1234567), "-1,234,567");
    }

    #[test]
    fn the_frame_fades_in_from_the_middle() {
        let l = motor(10);
        let f = l.frame(100, 0.0);
        assert_eq!((f.width, f.text), (0, false));
        let f = l.frame(100, 4.0);
        assert_eq!(f.width, l.width + 13);
        assert_eq!((f.x, f.y, f.height), (-11, -17, 22 + 33));
        assert!(f.text);
        assert_eq!(f.text_x, -11 + (l.width + 13) / 2);
        assert_eq!((f.title_y, f.tip_y), (-14, 22 + 33 - 27));
        // A title wider than the board widens the background both ways.
        let f = l.frame(l.width + 13, 4.0);
        assert_eq!((f.width, f.x), (l.width + 23, -11 - 5));
        assert_eq!(shade(0.0), 0x0010_1010);
        assert_eq!(shade(20.0), 0x5010_1010);
        assert!((MOTOR.pitch(0) - 1.15).abs() < 1e-6 && (MOTOR.pitch(256) - 1.5).abs() < 1e-6);
        assert_eq!(Layout::cursor([50.5, 5.0], 12), [43, -2, 15]);
    }

    #[test]
    fn the_board_opens_on_the_sixth_tick_held() {
        let mut h = Hold::default();
        for _ in 0..5 {
            assert_eq!(h.tick(true), Step::Wait);
        }
        assert_eq!(h.tick(true), Step::Open);
        assert_eq!(Hold::default().tick(false), Step::Short);
    }

    #[test]
    fn the_hover_tip_warms_up_then_fades() {
        let mut t = HoverTip::default();
        for _ in 0..3 {
            t.show();
            assert_eq!(t.alpha(), None);
        }
        t.show();
        assert_eq!((t.ticks, t.warmup), (11, 7));
        assert_eq!(t.alpha(), Some(0.0));
        t.tick();
        assert_eq!(t.alpha(), Some(0.2));
        for _ in 0..5 {
            t.tick();
        }
        assert_eq!(t.alpha(), Some(1.0));
        // The warm-up has run down meanwhile: looking again only warms it.
        t.show();
        assert_eq!((t.ticks, t.warmup), (5, 3));
    }
}
