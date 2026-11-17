//! Create 6's value settings board: hold use on a value box and a board of rows opens
//! (`ValueSettingsClient.tick`), the cursor picks a row and a column (`ValueSettingsScreen`),
//! releasing use sends the pick (`ValueSettingsPacket`) and the block's behaviour turns it into
//! its value (`ValueSettingsBehaviour.setValueSettings`). Layout numbers are GUI pixels; text
//! widths come from the game's font.

use crate::kinetics::{Axis, Dir};

/// The suppliers installed with `ScrollValueBehaviour.onlyActiveWhen` and
/// `FilteringBehaviour.onlyActiveWhen`. Activity belongs to the behaviour, not its geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Always,
    IdleValve,
    UndrivenSteamShaft,
    DistributingTunnel,
    BrassFilter,
}

impl Activity {
    /// Create's `addBehaviours` declarations on valve handles, steam engines, tunnels and funnels.
    pub fn of(block: crate::Block) -> Self {
        use crate::Block as B;
        match block {
            b if crate::dye::is_valve_handle(b) => Self::IdleValve,
            B::SteamEngine => Self::UndrivenSteamShaft,
            B::BrassTunnel => Self::DistributingTunnel,
            B::AndesiteFunnel | B::AndesiteBeltFunnel | B::BrassFunnel | B::BrassBeltFunnel => {
                Self::BrassFilter
            }
            _ => Self::Always,
        }
    }
}

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

/// The rotation speed controller's board (`SpeedControllerBlockEntity.targetSpeed`,
/// `create.kinetics.speed_controller.rotation_speed`).
pub const CONTROLLER: Board = kinetic("Targeted Speed in RPM");

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

/// A brass diode's board (`BrassDiodeBlockEntity`, `create.logistics.redstone_interval`).
pub const DIODE: Board = brass_diode("Redstone Interval");

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

/// `ChassisScrollValueBehaviour.setValueSettings` on a chassis whose range is `current`, within
/// `between(1, max_range)`; None when nothing is set. The base
/// `ScrollValueBehaviour.setValueSettings` compares the range picked (`value + 1`) with the
/// overridden `getValueSettings` (`current - 1`), so picking the range one below the current one
/// does nothing.
pub fn chassis_value(s: Settings, current: i32, max_range: i32) -> Option<i32> {
    let picked = Settings {
        row: s.row,
        value: s.value + 1,
    };
    (picked != chassis_settings(current)).then(|| picked.value.clamp(1, max_range))
}

/// The chassis board's formatter.
pub fn chassis_text(s: Settings) -> String {
    number(s.value + 1)
}

/// `ScrollOptionBehaviour` set to `s` while on option `current`, within `between(0, options - 1)`;
/// None when it is the current one (`ScrollValueBehaviour.setValueSettings` returns early).
pub fn option_value(s: Settings, current: i32, options: i32) -> Option<i32> {
    let now = Settings {
        row: 0,
        value: current,
    };
    (s != now).then(|| s.value.clamp(0, options - 1))
}

/// `ValueBoxTransform.Sided.getLocalOffset`: the slot at `south` (in the block, for the south
/// side) turned to `side` (`rotateCentered` by `horizontalAngle(side)` about Y, then by
/// `verticalAngle(side)` about X).
pub fn sided_offset(south: [f64; 3], side: Dir) -> [f64; 3] {
    let y_rot: f64 = match side {
        Dir::South | Dir::Up | Dir::Down => 0.0,
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
    };
    let x_rot: f64 = match side {
        Dir::Up => -90.0,
        Dir::Down => 90.0,
        _ => 0.0,
    };
    let [x, y, z] = [south[0] - 0.5, south[1] - 0.5, south[2] - 0.5];
    let (s, c) = y_rot.to_radians().sin_cos();
    let (x, z) = (x * c + z * s, z * c - x * s);
    let (s, c) = x_rot.to_radians().sin_cos();
    let (y, z) = (y * c - z * s, z * c + y * s);
    [x + 0.5, y + 0.5, z + 0.5]
}

/// `ValueBoxTransform.testHit`: the hit, in the block, within half the box's scale (.5) of its
/// slot.
pub fn hits_box(slot: [f64; 3], local: [f64; 3]) -> bool {
    let d2: f64 = (0..3).map(|i| (local[i] - slot[i]).powi(2)).sum();
    d2.sqrt() < 0.5 / 2.0
}

/// `CreativeMotorBlockEntity.MotorValueBox` on `side` of a motor facing `facing`: 12.5 px out on
/// a side off the motor's axis (not the bottom of a sideways motor), a pixel back from its
/// facing; None on a side without one (`isSideActive`).
pub fn motor_slot(facing: Dir, side: Dir) -> Option<[f64; 3]> {
    if facing.axis() != Axis::Y && side == Dir::Down || side.axis() == facing.axis() {
        return None;
    }
    let o = sided_offset([0.5, 0.5, 12.5 / 16.0], side);
    let n = facing.offset();
    Some([0, 1, 2].map(|i| o[i] - f64::from(n[i]) / 16.0))
}

/// `SpeedControllerBlockEntity.ControllerValueBoxTransform` on `side` of a controller along
/// `axis`: at (8, 11, 15.5) px on the two sides across its axis, none on the top or bottom.
pub fn controller_slot(axis: Axis, side: Dir) -> Option<[f64; 3]> {
    if side.axis() == Axis::Y || side.axis() == axis {
        return None;
    }
    Some(sided_offset([0.5, 11.0 / 16.0, 15.5 / 16.0], side))
}

/// `ValveHandleValueBox` on `side` of a valve handle facing `facing`: on its handle, 4.5 px out
/// of its facing side, the only side it has (`isSideActive`).
pub fn valve_slot(facing: Dir, side: Dir) -> Option<[f64; 3]> {
    (side == facing).then(|| sided_offset([0.5, 0.5, 4.5 / 16.0], side))
}

/// `ValveHandleValueBox.testHit`: within a third of a block (`scale / 1.5`) of the box.
pub fn hits_valve_box(slot: [f64; 3], local: [f64; 3]) -> bool {
    let d2: f64 = (0..3).map(|i| (local[i] - slot[i]).powi(2)).sum();
    d2.sqrt() < 0.5 / 1.5
}

/// `BrassTunnelModeSlot` (a `CenteredSideValueBoxTransform`): 15.5 px out, on the top only.
pub fn tunnel_mode_slot(side: Dir) -> Option<[f64; 3]> {
    (side == Dir::Up).then(|| centred_slot(side))
}

/// The option names of a contraption block's `ScrollOptionBehaviour`, in Create's lang order.
pub const ROTATION_MODES: [&str; 3] = [
    "Always Place when Stopped",
    "Only Place near Initial Angle",
    "Only Place when Anchor Destroyed",
];
pub const MOVEMENT_MODES: [&str; 3] = [
    "Always Place when Stopped",
    "Place only in Starting Position",
    "Place only when Anchor Destroyed",
];
pub const ROTATION_DIRECTIONS: [&str; 2] = ["Clockwise", "Counter-Clockwise"];
pub const CLOCK_HANDS: [&str; 3] = ["Hour hand first", "Minute hand first", "24-Hour hand first"];
pub const ROLLER_MODES: [&str; 3] = [
    "Clear Blocks and Pave",
    "Straight Fill Below",
    "Sloped Fill Below",
];

/// `CenteredSideValueBoxTransform`: 15.5 px out, in the middle of `side`.
pub fn centred_slot(side: Dir) -> [f64; 3] {
    sided_offset([0.5, 0.5, 15.5 / 16.0], side)
}

/// `DirectionalExtenderScrollOptionSlot` (bearings' and pistons' movement mode): centred on a
/// side off the block's facing axis (and, for a piston, off its shaft's `shaft` axis), 2 px back
/// from its facing; None on any other side.
pub fn extender_slot(facing: Dir, shaft: Option<Axis>, side: Dir) -> Option<[f64; 3]> {
    if side.axis() == facing.axis() || Some(side.axis()) == shaft {
        return None;
    }
    let o = centred_slot(side);
    let n = facing.offset();
    Some([0, 1, 2].map(|i| o[i] - f64::from(n[i]) * 2.0 / 16.0))
}

/// `ArmBlockEntity.SelectionModeValueBox` (a `CenteredSideValueBoxTransform` on the sides
/// only): 15.5 px out, 3 px up (13 under a ceiling arm).
pub fn arm_mode_slot(ceiling: bool, side: Dir) -> Option<[f64; 3]> {
    if side.axis() == Axis::Y {
        return None;
    }
    let y = if ceiling { 13.0 } else { 3.0 };
    Some(sided_offset([0.5, y / 16.0, 15.5 / 16.0], side))
}

/// The arm's selection modes (`create.mechanical_arm.selection_mode.*`), in order.
pub const ARM_MODES: [&str; 3] = ["Round Robin", "Forced Round Robin", "Prefer First Target"];

/// The arm's board (`create.logistics.when_multiple_outputs_available`).
pub const ARM_MODE: Board = options("Distribution Method", ARM_MODES.len() as i32);

/// `EjectorBlockEntity.EjectorSlot` for an ejector facing `facing`: on its two sides along its
/// facing, 6 px up and 15.5 out; on its top while its plate is not charged, 5 px behind the
/// middle, 10.5 px up.
pub fn ejector_slot(facing: Dir, side: Dir, charged: bool) -> Option<[f64; 3]> {
    if side == Dir::Up {
        if charged {
            return None;
        }
        let angle = f64::from(crate::ejector::horizontal_angle(facing));
        let [x, _, z] = crate::ejector::rotate_y([0.0, 0.0, -5.0 / 16.0], angle);
        return Some([0.5 + x, 10.5 / 16.0, 0.5 + z]);
    }
    if side.axis() != facing.axis() || side.axis() == Axis::Y {
        return None;
    }
    Some(sided_offset([0.5, 6.0 / 16.0, 15.5 / 16.0], side))
}

/// The ejector's board (`create.weighted_ejector.stack_size`, `between(0, 64)`).
pub const EJECTOR: Board = scroll_value("Ejected Stack Size", 64);

/// `FactoryPanelBehaviour.createBoard`: the target amount, up to 100 items (row 0) or stacks
/// (row 1), a milestone every 10.
pub const FACTORY: Board = Board {
    title: "Target Amount in Storage",
    max: 100,
    milestone: 10,
    rows: &["Items", "Stacks"],
    icons: false,
};

/// `FactoryPanelBehaviour.formatValue`: ` Inactive ` at 0, stacks marked `\u{25a4}`.
pub fn factory_text(s: Settings) -> String {
    if s.value == 0 {
        " Inactive ".to_owned()
    } else {
        format!(
            "{}{}",
            s.value.max(0),
            if s.row == 0 { "" } else { "\u{25a4}" }
        )
    }
}

/// The ejector's stack size as its value box shows it: `*` for any.
pub fn ejector_text(s: Settings) -> String {
    if s.value == 0 {
        "*".to_owned()
    } else {
        s.value.to_string()
    }
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

/// `logistics.filter.extracted_amount`: a filter slot's board's title.
/// `TableClothFilteringBehaviour.createBoard`: a shop's price, 1 to 100 of its payment item
/// (`table_cloth.price_per_order`, one row `table_cloth.amount`), a milestone every 10.
pub const TABLE_CLOTH: Board = Board {
    title: "Price per Order",
    max: 100,
    milestone: 10,
    rows: &["Amount"],
    icons: false,
};

pub const FILTER_TITLE: &str = "Extracted Amount";

/// `FilteringBehaviour.createBoard`: "Up to" and "Exactly" rows of 0 to `max` (the filter's
/// stack size, `getMaxStackSize`), a milestone every 16.
pub const fn filter(max: i32) -> Board {
    Board {
        title: FILTER_TITLE,
        max,
        milestone: 16,
        rows: &["Up to", "Exactly"],
        icons: false,
    }
}

/// `FilteringBehaviour`'s `count` and `upTo`: how much a brass funnel or smart chute takes at
/// once (`FilterAmount`, `UpTo`). Without a setting (`count` 0, as read from an empty save) it
/// is up to the filter's stack size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterAmount {
    /// 0: the filter's stack size.
    pub count: u8,
    pub up_to: bool,
}

/// `FilteringBehaviour`'s `count = 64, upTo = true`: up to a stack.
impl Default for FilterAmount {
    fn default() -> FilterAmount {
        FilterAmount {
            count: 0,
            up_to: true,
        }
    }
}

impl FilterAmount {
    /// `FilteringBehaviour.read`: `count` 0 is up to a stack.
    pub fn new(count: u8, up_to: bool) -> FilterAmount {
        FilterAmount {
            count,
            up_to: up_to || count == 0,
        }
    }

    /// The count, `max` (the filter's stack size) for none.
    pub fn count(self, max: i32) -> i32 {
        if self.count == 0 {
            max
        } else {
            i32::from(self.count)
        }
    }

    /// `getValueSettings`.
    pub fn settings(self, max: i32) -> Settings {
        Settings {
            row: if self.up_to { 0 } else { 1 },
            value: self.count(max),
        }
    }

    /// `setValueSettings`: `count` within 1 and `max`, "Up to" on the first row.
    pub fn from_settings(s: Settings, max: i32) -> FilterAmount {
        FilterAmount {
            count: s.value.clamp(1, max.clamp(1, 255)) as u8,
            up_to: s.row == 0,
        }
    }

    /// `setFilter`: an exact count fits the new filter's stack size (`max`).
    pub fn fitted(self, max: i32) -> FilterAmount {
        if self.up_to || self.count == 0 {
            return self;
        }
        FilterAmount {
            count: (i32::from(self.count).min(max.max(1))) as u8,
            ..self
        }
    }

    /// The value a `create:value` message carries for it: the row in the second byte.
    pub fn word(self, max: i32) -> i32 {
        let s = self.settings(max);
        s.row << 8 | s.value
    }
}

/// The settings a `create:value` word for a filter slot carries (`FilterAmount::word`).
pub fn filter_settings(word: i32) -> Settings {
    Settings {
        row: (word >> 8) & 1,
        value: word & 0xFF,
    }
}

/// `FilteringBehaviour.formatValue`: "Any" for up to a whole stack, else `≤n` or `=n`.
pub fn filter_text(s: Settings, max: i32) -> String {
    if s.row == 0 && s.value == max {
        return "Any".to_owned();
    }
    let sign = if s.row == 0 { '\u{2264}' } else { '=' };
    format!("{sign}{}", s.value.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_amounts_round_trip_through_the_board() {
        let any = FilterAmount::default();
        assert_eq!(any.settings(64), Settings { row: 0, value: 64 });
        assert_eq!(filter_text(any.settings(64), 64), "Any");
        assert_eq!(filter_text(any.settings(16), 16), "Any");
        let exact = FilterAmount::from_settings(Settings { row: 1, value: 0 }, 64);
        assert_eq!(exact, FilterAmount::new(1, false));
        assert_eq!(filter_text(exact.settings(64), 64), "=1");
        let up = FilterAmount::from_settings(Settings { row: 0, value: 20 }, 64);
        assert_eq!(filter_text(up.settings(64), 64), "\u{2264}20");
        assert_eq!(filter_settings(up.word(64)), up.settings(64));
        assert_eq!(filter_settings(exact.word(64)), exact.settings(64));
        // An exact count fits a smaller stack; up to stays.
        assert_eq!(FilterAmount::new(40, false).fitted(16).count, 16);
        assert_eq!(FilterAmount::new(40, true).fitted(16).count, 40);
        assert!(FilterAmount::new(0, false).up_to);
        let b = filter(64);
        assert_eq!((b.milestone_count(), b.scale()), (5, 2));
    }

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
        let range = |value| Settings { row: 0, value };
        assert_eq!(chassis_value(range(15), 4, 16), Some(16));
        assert_eq!(chassis_value(range(0), 4, 16), Some(1));
        assert_eq!(chassis_value(range(3), 4, 16), Some(4));
        // Create compares range + 1 with current - 1: range 3 on a chassis at 4 does nothing.
        assert_eq!(chassis_value(range(2), 4, 16), None);
        assert_eq!(chassis_text(Settings { row: 0, value: 0 }), "1");
        assert_eq!(option_value(Settings { row: 0, value: 9 }, 0, 7), Some(6));
        assert_eq!(option_value(Settings { row: 0, value: 2 }, 2, 7), None);
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
    fn movement_mode_slots_sit_off_the_facing_and_back_two_pixels() {
        // A bearing facing up: on its sides, 15.5 px out, 2 px down; none on top or bottom.
        assert_eq!(extender_slot(Dir::Up, None, Dir::Up), None);
        assert_eq!(extender_slot(Dir::Up, None, Dir::Down), None);
        let north = extender_slot(Dir::Up, None, Dir::North).unwrap();
        let close = |a: [f64; 3], b: [f64; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9);
        assert!(close(north, [0.5, 0.375, 0.5 / 16.0]));
        // A piston facing east turning a z shaft: its box is on top and bottom only.
        assert_eq!(extender_slot(Dir::East, Some(Axis::Z), Dir::North), None);
        let top = extender_slot(Dir::East, Some(Axis::Z), Dir::Up).unwrap();
        assert!(close(top, [0.375, 15.5 / 16.0, 0.5]));
        assert!(close(centred_slot(Dir::Up), [0.5, 15.5 / 16.0, 0.5]));
    }

    #[test]
    fn value_boxes_sit_on_their_sides() {
        let near = |a: [f64; 3], b: [f64; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9);
        let px = |x: f64, y: f64, z: f64| [x / 16.0, y / 16.0, z / 16.0];
        // The south location turned onto each face.
        let south = px(8.0, 8.0, 12.5);
        assert!(near(sided_offset(south, Dir::South), south));
        assert!(near(sided_offset(south, Dir::North), px(8.0, 8.0, 3.5)));
        assert!(near(sided_offset(south, Dir::West), px(3.5, 8.0, 8.0)));
        assert!(near(sided_offset(south, Dir::East), px(12.5, 8.0, 8.0)));
        assert!(near(sided_offset(south, Dir::Up), px(8.0, 12.5, 8.0)));
        assert!(near(sided_offset(south, Dir::Down), px(8.0, 3.5, 8.0)));
        // A motor facing north: boxes on top and the sides, a pixel toward its back.
        let top = motor_slot(Dir::North, Dir::Up).unwrap();
        assert!(near(top, px(8.0, 12.5, 9.0)));
        assert_eq!(motor_slot(Dir::North, Dir::Down), None);
        assert_eq!(motor_slot(Dir::North, Dir::South), None);
        assert!(motor_slot(Dir::Up, Dir::East).is_some());
        assert_eq!(motor_slot(Dir::Up, Dir::Down), None);
        let south = controller_slot(Axis::X, Dir::South).unwrap();
        assert!((south[1] - 11.0 / 16.0).abs() < 1e-9 && (south[2] - 15.5 / 16.0).abs() < 1e-9);
        let west = controller_slot(Axis::Z, Dir::West).unwrap();
        assert!((west[0] - 0.5 / 16.0).abs() < 1e-9 && (west[2] - 0.5).abs() < 1e-9);
        assert_eq!(controller_slot(Axis::X, Dir::East), None);
        assert_eq!(controller_slot(Axis::X, Dir::Up), None);
        assert!(hits_box(top, px(8.0, 12.5, 12.9)));
        assert!(!hits_box(top, px(8.0, 12.5, 13.0)));
        // A brass tunnel's mode is picked on its top.
        assert!(near(tunnel_mode_slot(Dir::Up).unwrap(), px(8.0, 15.5, 8.0)));
        assert_eq!(tunnel_mode_slot(Dir::East), None);
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
