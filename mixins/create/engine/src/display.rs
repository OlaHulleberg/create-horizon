//! Create's display links (`DisplayLinkBlock`, `DisplayLinkBlockEntity`, `DisplaySource`,
//! `DisplayTarget` and their subclasses), the targets they write to (nixie tubes, signs, display
//! boards: `NixieTubeBlockEntity`, `FlapDisplayBlockEntity`, `FlapDisplayLayout`,
//! `FlapDisplaySection`) and the text each source makes, formatted as Create formats it.
//!
//! A link refreshes only when its source's interval comes round or a signal ends (no counter
//! ticks per link): the links keep the game tick each one is next due at, sorted.

use crate::Block;
use crate::kinetics::Dir;
use std::collections::{BTreeSet, HashMap};

/// Nixie tubes' and display boards' text, server to clients.
pub const CHANNEL: &str = "create:display";

// ---------------------------------------------------------------------------------------------
// Language

/// The en_us text of the lang keys used here (Create's generated `en_us.json`, MIT), for when the
/// game's own lang has no entry.
pub fn en_us(key: &str) -> Option<&'static str> {
    Some(match key {
        "create.generic.unit.rpm" => "RPM",
        "create.generic.unit.stress" => "su",
        "create.generic.daytime.am" => "am",
        "create.generic.daytime.pm" => "pm",
        "create.flap_display.cycles.alphabet" => {
            " ;A;B;C;D;E;F;G;H;I;J;K;L;M;N;O;P;Q;R;S;T;U;V;X;Y;Z"
        }
        "create.flap_display.cycles.numeric" => " ;0;1;2;3;4;5;6;7;8;9",
        "create.flap_display.cycles.instant" => " ; ",
        "create.flap_display.cycles.pixel" => "\u{2588};\u{2593};\u{2592}",
        "create.display_source.kinetic_speed" => "Rotation Speed (RPM)",
        "create.display_source.kinetic_stress" => "Network Stress",
        "create.display_source.redstone_power" => "Redstone Power",
        "create.display_source.time_of_day" => "Time of Day",
        "create.display_source.stop_watch" => "Stopwatch",
        "create.display_source.combine_item_names" => "Combine Item Names",
        "create.display_source.nixie_tube" => "Copy Nixie Tubes",
        "create.display_link.success" => "Successfully bound to targeted position",
        "create.display_link.invalid" => "Link has no valid target, try placing it again",
        "create.display_link.too_far" => "Targeted position is too far from here",
        "create.display_link.no_source" => "Not a Display Source",
        _ => return None,
    })
}

/// A lang key's text in the game's language, if it has one.
pub type Resolve<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Lang lookups: the game's language first (`resolve`), else en_us, else the key itself
/// (`Component.translatable` of an unknown key).
pub struct Lang<'a> {
    pub resolve: Option<Resolve<'a>>,
}

impl Lang<'_> {
    pub const EN_US: Lang<'static> = Lang { resolve: None };

    pub fn get(&self, key: &str) -> String {
        self.resolve
            .and_then(|r| r(key))
            .or_else(|| en_us(key).map(str::to_owned))
            .unwrap_or_else(|| key.to_owned())
    }
}

/// `CreateLang.number` (`LangNumberFormat.format`): `Mth.equal(d, 0)` reads as 0, then grouped
/// thousands and at most two decimals.
pub fn number(v: f64) -> String {
    let v = if (v as f32).abs() < 1.0e-5 { 0.0 } else { v };
    crate::gauge::number(v)
}

// ---------------------------------------------------------------------------------------------
// Sources

/// The display sources the port has the blocks for (`AllDisplaySources`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    KineticSpeed,
    KineticStress,
    RedstonePower,
    TimeOfDay,
    StopWatch,
    ItemNames,
    NixieTube,
}

impl Source {
    pub const ALL: [Source; 7] = [
        Source::KineticSpeed,
        Source::KineticStress,
        Source::RedstonePower,
        Source::TimeOfDay,
        Source::StopWatch,
        Source::ItemNames,
        Source::NixieTube,
    ];

    /// Its registry id's path (`create:<id>`).
    pub fn id(self) -> &'static str {
        match self {
            Source::KineticSpeed => "kinetic_speed",
            Source::KineticStress => "kinetic_stress",
            Source::RedstonePower => "redstone_power",
            Source::TimeOfDay => "time_of_day",
            Source::StopWatch => "stopwatch",
            Source::ItemNames => "item_names",
            Source::NixieTube => "nixie_tube",
        }
    }

    /// `DisplaySource.get`: by id, with or without `create:`, legacy names included.
    pub fn from_id(id: &str) -> Option<Source> {
        let id = id.strip_prefix("create:").unwrap_or(id);
        Source::ALL.into_iter().find(|s| s.id() == id).or(match id {
            "redstone_power_display_source" => Some(Source::RedstonePower),
            "nixie_tube_source" => Some(Source::NixieTube),
            "belt_source_combine_item_names"
            | "depot_source_combine_item_names"
            | "weighted_ejector_source_combine_item_names" => Some(Source::ItemNames),
            "cuckoo_clock_source_time_of_day" => Some(Source::TimeOfDay),
            "cuckoo_clock_source_stop_watch" => Some(Source::StopWatch),
            "speedometer_source_kinetic_speed" => Some(Source::KineticSpeed),
            "stressometer_source_kinetic_stress" => Some(Source::KineticStress),
            _ => None,
        })
    }

    /// `getName`'s lang key (`create.display_source.` + `getTranslationKey`).
    pub fn name_key(self) -> &'static str {
        match self {
            Source::KineticSpeed => "create.display_source.kinetic_speed",
            Source::KineticStress => "create.display_source.kinetic_stress",
            Source::RedstonePower => "create.display_source.redstone_power",
            Source::TimeOfDay => "create.display_source.time_of_day",
            Source::StopWatch => "create.display_source.stop_watch",
            Source::ItemNames => "create.display_source.combine_item_names",
            Source::NixieTube => "create.display_source.nixie_tube",
        }
    }

    /// `getPassiveRefreshTicks`: 100, the stopwatch 20 (every source here resets passively).
    pub fn refresh_ticks(self) -> i64 {
        match self {
            Source::StopWatch => 20,
            _ => 100,
        }
    }

    /// `SingleLineDisplaySource.allowsLabeling`.
    pub fn allows_labeling(self, target: Target) -> bool {
        match self {
            Source::NixieTube => target != Target::Nixie,
            _ => true,
        }
    }
}

/// The settings a link's screen gives its source (`sourceConfig`): `Label`, `Directional`
/// (speed), `Mode` (stress, redstone power), `Cycle` (time of day), `StartTime` (stopwatch).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub label: String,
    pub directional: i32,
    pub mode: i32,
    pub cycle: i32,
    pub start_time: Option<i64>,
}

impl Config {
    /// One `Key=value` of the screen; false for a key no source here reads.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let int = || value.parse::<i32>().unwrap_or(0);
        match key {
            "Label" => self.label = value.to_owned(),
            "Directional" => self.directional = int(),
            "Mode" => self.mode = int(),
            "Cycle" => self.cycle = int(),
            _ => return false,
        }
        true
    }
}

/// The display targets the port has (`AllDisplayTargets`; lecterns not yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Nixie,
    Sign { hanging: bool },
    Board,
}

/// `DisplayTargetStats`: rows and columns a target shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub rows: i32,
    pub columns: i32,
}

/// What a source reads from the world at a refresh, typed, so a refresh whose reading didn't
/// change formats nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading {
    /// A speedometer's speed (None: not a speedometer).
    Speed(Option<f32>),
    /// A stressometer's network stress and capacity.
    Stress(Option<(f32, f32)>),
    /// `max(getDirectSignalTo(source), its POWER property)`.
    Power(i32),
    /// The level's day time, whether its dimension is natural, whether the cuckoo clock turns,
    /// and the two random numbers an unnatural dimension shows (`nextInt(70)`, `nextInt(40)`).
    Time {
        day_time: i64,
        natural: bool,
        running: bool,
        random: [i32; 2],
    },
    /// The game time and whether the cuckoo clock turns.
    Watch { now: i64, running: bool },
    /// Each item name along the line of depots, belts and ejectors (`ItemNameDisplaySource`).
    Names(Vec<String>),
    /// A nixie tube's full text (None: not a nixie tube).
    Nixie(Option<String>),
}

/// One line a source gives: None is `EMPTY_LINE`; `numeric` is `flapDisplayContext` (a nixie
/// tube's text that reads as an integer).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: Option<String>,
    pub numeric: bool,
}

/// `TimeOfDayDisplaySource.EMPTY_TIME`.
pub const EMPTY_TIME: &str = "--:--";

/// `PercentOrProgressBarDisplaySource.provideLine`'s bar, or the number.
fn percent_or_bar(
    source: Source,
    config: &Config,
    progress: Option<f32>,
    target: Target,
    stats: Stats,
    lang: &Lang,
) -> Option<String> {
    let raw = progress?;
    let bar = match source {
        Source::KineticStress => config.mode == 0,
        _ => config.mode != 0,
    };
    if !bar {
        return Some(match source {
            // RedstonePowerDisplaySource.formatNumeric.
            Source::RedstonePower => ((raw * 15.0) as i32).to_string(),
            // KineticStressDisplaySource.formatNumeric: mode 1 is the percentage.
            _ if config.mode == 1 => format!("{}%", ((raw * 100.0) as i32).clamp(0, 100)),
            _ => {
                let space = if target == Target::Board { " " } else { "" };
                format!(
                    "{}{space}{}",
                    number(f64::from(raw)),
                    lang.get("create.generic.unit.stress")
                )
            }
        });
    }
    let label_size = if config.label.is_empty() {
        0
    } else {
        config.label.chars().count() as i32 + 1
    };
    let mut length = (stats.columns - label_size).min(128);
    if matches!(target, Target::Sign { .. }) {
        length = (length as f32 * 6.0 / 9.0) as i32;
    }
    if target == Target::Board {
        // sizeForWideChars: MONOSPACE / WIDE_MONOSPACE.
        length = (length as f32 * MONOSPACE / WIDE_MONOSPACE) as i32;
    }
    let level = raw.clamp(0.0, 1.0);
    let filled = (level * length as f32) as i32;
    if length < 1 {
        return None;
    }
    let empty = length - filled;
    let mut s = "\u{2588}".repeat(filled.max(0) as usize);
    s.push_str(&"\u{2592}".repeat(empty.max(0) as usize));
    Some(s)
}

/// `provideLine` of each source, from its reading. The stopwatch starts on its first reading
/// (`onSignalReset` when `StartTime` is missing).
pub fn provide_line(
    source: Source,
    config: &mut Config,
    reading: &Reading,
    target: Target,
    stats: Stats,
    lang: &Lang,
) -> Line {
    let mut numeric = false;
    let text = match (source, reading) {
        // KineticSpeedDisplaySource: `number(speed) RPM`, absolute unless `Directional`.
        (Source::KineticSpeed, Reading::Speed(speed)) => Some(match speed {
            None => "0".to_owned(),
            Some(s) => {
                let s = if config.directional == 0 { s.abs() } else { *s };
                format!(
                    "{} {}",
                    number(f64::from(s)),
                    lang.get("create.generic.unit.rpm")
                )
            }
        }),
        // KineticStressDisplaySource.getProgress.
        (Source::KineticStress, Reading::Stress(stress)) => {
            let progress = stress.map(|(stress, capacity)| {
                if capacity == 0.0 {
                    return 0.0;
                }
                match config.mode {
                    0 | 1 => stress / capacity,
                    2 => stress,
                    3 => capacity,
                    4 => capacity - stress,
                    _ => 0.0,
                }
            });
            percent_or_bar(source, config, progress, target, stats, lang)
        }
        (Source::RedstonePower, Reading::Power(p)) => {
            percent_or_bar(source, config, Some(*p as f32 / 15.0), target, stats, lang)
        }
        (
            Source::TimeOfDay,
            Reading::Time {
                day_time,
                natural,
                running,
                random,
            },
        ) => Some(if *running {
            time_of_day(*day_time, *natural, config.cycle == 0, *random, lang)
        } else {
            EMPTY_TIME.to_owned()
        }),
        (Source::StopWatch, Reading::Watch { now, running }) => Some(if *running {
            let started = *config.start_time.get_or_insert(*now);
            stopwatch(*now - started)
        } else {
            EMPTY_TIME.to_owned()
        }),
        // ItemNameDisplaySource: the names one after another (never EMPTY_LINE itself).
        (Source::ItemNames, Reading::Names(names)) => Some(names.concat()),
        (Source::NixieTube, Reading::Nixie(text)) => {
            // `Integer.valueOf(line)`: a sign, then decimal digits that fit an int.
            numeric = text.as_deref().is_some_and(|t| t.parse::<i32>().is_ok());
            text.clone()
        }
        _ => None,
    };
    Line { text, numeric }
}

/// `TimeOfDayDisplaySource.provideLine` for a turning clock: 12-hour (`c12`) with am/pm, or
/// 24-hour; minutes in fives; nonsense in a dimension without a day.
pub fn time_of_day(
    day_time: i64,
    natural: bool,
    c12: bool,
    random: [i32; 2],
    lang: &Lang,
) -> String {
    let day = (day_time % 24000) as i32;
    let mut hours = (day / 1000 + 6) % 24;
    let mut minutes = (day % 1000) * 60 / 1000;
    let suffix = lang.get(if hours > 11 {
        "create.generic.daytime.pm"
    } else {
        "create.generic.daytime.am"
    });
    minutes = minutes / 5 * 5;
    if c12 {
        hours %= 12;
        if hours == 0 {
            hours = 12;
        }
    }
    if !natural {
        hours = random[0] + 24;
        minutes = random[1] + 60;
    }
    let mut s = format!(
        "{}{hours}:{}{minutes}{}",
        if hours < 10 { " " } else { "" },
        if minutes < 10 { "0" } else { "" },
        if c12 { " " } else { "" }
    );
    if c12 {
        s.push_str(&suffix);
    }
    s
}

/// `StopWatchDisplaySource.provideLine`: hours only once there are any.
pub fn stopwatch(diff: i64) -> String {
    let diff = diff as i32;
    let hours = diff / 60 / 60 / 20;
    let minutes = (diff / 60 / 20) % 60;
    let seconds = (diff / 20) % 60;
    let head = if hours == 0 {
        String::new()
    } else {
        format!("{}{hours}:", if hours < 10 { " " } else { "" })
    };
    let pad = if minutes < 10 {
        if hours == 0 { " " } else { "0" }
    } else {
        ""
    };
    format!(
        "{head}{pad}{minutes}:{}{seconds}",
        if seconds < 10 { "0" } else { "" }
    )
}

/// `SingleLineDisplaySource.provideText`: the label before the line, `EMPTY` for no line.
pub fn provide_text(source: Source, config: &Config, line: &Line, target: Target) -> Vec<String> {
    let Some(text) = &line.text else {
        return vec![String::new()];
    };
    if source.allows_labeling(target) && !config.label.is_empty() {
        return vec![format!("{} {text}", config.label)];
    }
    vec![text.clone()]
}

/// `SingleLineDisplaySource.provideFlapDisplayText`: the label and the value as two sections.
pub fn provide_flap_text(source: Source, config: &Config, line: &Line) -> Vec<Vec<String>> {
    if source.allows_labeling(Target::Board) && !config.label.is_empty() {
        return vec![vec![
            format!("{} ", config.label),
            line.text.clone().unwrap_or_default(),
        ]];
    }
    provide_text(source, config, line, Target::Board)
        .into_iter()
        .map(|t| vec![t])
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Display boards

/// `FlapDisplaySection.MONOSPACE`, `WIDE_MONOSPACE`.
pub const MONOSPACE: f32 = 7.0;
pub const WIDE_MONOSPACE: f32 = 9.0;

/// The flap cycles the sections here use (`create.flap_display.cycles.<key>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cycle {
    Alphabet,
    Numeric,
    Instant,
    Pixel,
}

impl Cycle {
    pub const ALL: [Cycle; 4] = [
        Cycle::Alphabet,
        Cycle::Numeric,
        Cycle::Instant,
        Cycle::Pixel,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Cycle::Alphabet => "alphabet",
            Cycle::Numeric => "numeric",
            Cycle::Instant => "instant",
            Cycle::Pixel => "pixel",
        }
    }

    pub fn from_key(key: &str) -> Option<Cycle> {
        Cycle::ALL.into_iter().find(|c| c.key() == key)
    }

    /// `getFlapCycle`: the lang entry split at `;`.
    pub fn options(self, lang: &Lang) -> Vec<String> {
        lang.get(&format!("create.flap_display.cycles.{}", self.key()))
            .split(';')
            .map(str::to_owned)
            .collect()
    }
}

/// `FlapDisplaySection`: its width in pixels, its cycle, its flags and the text it was given
/// (`component`); `transition` is `sendTransition`.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub size: f32,
    pub cycle: Cycle,
    pub single_flap: bool,
    pub has_gap: bool,
    pub right_aligned: bool,
    pub wide: bool,
    pub text: Option<String>,
    pub transition: bool,
}

impl Section {
    pub fn new(size: f32, cycle: Cycle, single_flap: bool, has_gap: bool) -> Section {
        Section {
            size,
            cycle,
            single_flap,
            has_gap,
            right_aligned: false,
            wide: false,
            text: None,
            transition: false,
        }
    }

    pub fn wide(mut self) -> Section {
        self.wide = true;
        self
    }

    /// `spinning.length`: one flap a monospace character, or one for a single flap.
    pub fn flaps(&self) -> usize {
        if self.single_flap {
            1
        } else {
            ((self.size / MONOSPACE) as i32).max(0) as usize
        }
    }

    /// `setText`.
    pub fn set_text(&mut self, text: &str) {
        self.text = Some(text.to_owned());
        self.transition = true;
    }
}

/// `FlapDisplayLayout`: a line's sections under a layout key.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub key: String,
    pub sections: Vec<Section>,
}

impl Layout {
    /// `new FlapDisplayLayout(maxCharCount)`.
    pub fn new(max_chars: i32) -> Layout {
        let mut l = Layout {
            key: String::new(),
            sections: Vec::new(),
        };
        l.load_default(max_chars);
        l
    }

    /// `loadDefault`: one alphabet section across the line.
    pub fn load_default(&mut self, max_chars: i32) {
        self.configure(
            "Default",
            vec![Section::new(
                max_chars as f32 * MONOSPACE,
                Cycle::Alphabet,
                false,
                false,
            )],
        );
    }

    pub fn is_layout(&self, key: &str) -> bool {
        self.key == key
    }

    pub fn configure(&mut self, key: &str, sections: Vec<Section>) {
        self.key = key.to_owned();
        self.sections = sections;
    }
}

/// A display board block's own state (`FlapDisplayBlockEntity`); only a controller has lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Board {
    pub controller: bool,
    pub x_size: i32,
    pub y_size: i32,
    /// None until asked for (`lines = null`, `initDefaultSections`).
    pub lines: Option<Vec<Layout>>,
    /// Each line's dye (`DyeColor` id; None white), glow and manual text.
    pub colour: Vec<Option<u8>>,
    pub glowing: Vec<bool>,
    pub manual: Vec<bool>,
}

impl Default for Board {
    fn default() -> Board {
        Board {
            controller: false,
            x_size: 1,
            y_size: 1,
            lines: None,
            colour: vec![None; 2],
            glowing: vec![false; 2],
            manual: vec![false; 2],
        }
    }
}

impl Board {
    /// `getMaxCharCount(gaps)`.
    pub fn max_chars_with(&self, gaps: i32) -> i32 {
        ((self.x_size as f32 * 16.0 - 2.0 - 4.0 * gaps as f32) / 3.5) as i32
    }

    pub fn max_chars(&self) -> i32 {
        self.max_chars_with(0)
    }

    /// `getLines`.
    pub fn lines(&mut self) -> &mut Vec<Layout> {
        let max = self.max_chars();
        let n = (self.y_size * 2) as usize;
        self.lines
            .get_or_insert_with(|| (0..n).map(|_| Layout::new(max)).collect())
    }

    /// `DisplayBoardTarget.provideStats`.
    pub fn stats(&self) -> Stats {
        Stats {
            rows: self.y_size * 2,
            columns: self.max_chars(),
        }
    }

    /// `updateControllerStatus` once the new values are known: true when anything changed.
    pub fn set_status(&mut self, controller: bool, x_size: i32, y_size: i32) -> bool {
        if self.controller == controller && self.x_size == x_size && self.y_size == y_size {
            return false;
        }
        self.controller = controller;
        self.x_size = x_size;
        self.y_size = y_size;
        let n = (y_size * 2) as usize;
        self.colour.resize(n, None);
        self.glowing.resize(n, false);
        self.manual = vec![false; n];
        self.lines = None;
        true
    }
}

/// A display board's block state, as far as its rules look at it (`HORIZONTAL_FACING`, `UP`,
/// `DOWN`, `WATERLOGGED`); two are the same state when all four match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardState {
    pub facing: Dir,
    pub up: bool,
    pub down: bool,
    pub waterlogged: bool,
}

/// `Direction.getClockWise` about y (horizontal directions only).
pub fn clockwise(d: Dir) -> Dir {
    match d {
        Dir::North => Dir::East,
        Dir::East => Dir::South,
        Dir::South => Dir::West,
        Dir::West => Dir::North,
        other => other,
    }
}

fn step(pos: [i32; 3], d: Dir, n: i32) -> [i32; 3] {
    let o = d.offset();
    [pos[0] + o[0] * n, pos[1] + o[1] * n, pos[2] + o[2] * n]
}

/// `FlapDisplayBlock.updateColumn`: whether a board facing `facing` at `pos` joins the boards
/// below and above it: every board of its row, both ways, has one there too. `facing_at` gives
/// the facing of the board at a position (None for anything else or an unloaded block).
pub fn column_connections(
    pos: [i32; 3],
    facing: Dir,
    facing_at: impl Fn([i32; 3]) -> Option<Dir>,
) -> (bool, bool) {
    let axis = clockwise(facing).axis();
    let joins = |at: [i32; 3]| at == pos || facing_at(at) == Some(facing);
    let mut out = [true, true];
    for (i, connection) in [Dir::Down, Dir::Up].into_iter().enumerate() {
        'moves: for movement in [Dir::from_axis(axis, false), Dir::from_axis(axis, true)] {
            let mut current = pos;
            for _ in 0..1000 {
                let col1 = joins(current);
                let col2 = facing_at(step(current, connection, 1)) == Some(facing);
                current = step(current, movement, 1);
                if !col1 && !col2 {
                    break;
                }
                if col1 && col2 {
                    continue;
                }
                out[i] = false;
                break 'moves;
            }
        }
    }
    (out[0], out[1])
}

/// `FlapDisplayBlockEntity.updateControllerStatus`'s new values: a board with none above it and
/// not continuing the one on its left controls the boards right of it (up to 32) and those
/// joined below it (up to 32).
pub fn controller_status(
    pos: [i32; 3],
    state: BoardState,
    state_at: impl Fn([i32; 3]) -> Option<BoardState>,
) -> (bool, i32, i32) {
    let left = clockwise(state.facing);
    let controller = !state.up && state_at(step(pos, left, 1)) != Some(state);
    let (mut x, mut y) = (1, 1);
    if controller {
        for xo in 1..32 {
            if state_at(step(pos, left.opposite(), xo)) != Some(state) {
                break;
            }
            x += 1;
        }
        for yo in 0..32 {
            if !state_at(step(pos, Dir::Down, yo)).is_some_and(|s| s.down) {
                break;
            }
            y += 1;
        }
    }
    (controller, x, y)
}

/// `FlapDisplayBlockEntity.getController`'s walk: up the joined boards, then along the row,
/// to the position of the board that may be the controller.
pub fn controller_pos(
    pos: [i32; 3],
    facing: Dir,
    state_at: impl Fn([i32; 3]) -> Option<BoardState>,
) -> Option<[i32; 3]> {
    let side = clockwise(facing);
    let mut cur = pos;
    for _ in 0..64 {
        if state_at(cur).is_some_and(|s| s.up) {
            cur = step(cur, Dir::Up, 1);
            continue;
        }
        if state_at(step(cur, side, 1)).is_some_and(|s| !s.up) {
            cur = step(cur, side, 1);
            continue;
        }
        return Some(cur);
    }
    None
}

/// `SingleLineDisplaySource.loadFlapDisplayLayout` (and `NumericSingleLineDisplaySource`,
/// `PercentOrProgressBarDisplaySource`, the clock sources' and the nixie source's overrides):
/// the line's layout for this source, kept when it already is that layout.
pub fn load_flap_layout(
    layout: &mut Layout,
    source: Source,
    config: &Config,
    numeric: bool,
    max_chars: i32,
) {
    let progress_bar = match source {
        Source::KineticStress => config.mode == 0,
        Source::RedstonePower => config.mode != 0,
        _ => false,
    };
    let key = match source {
        Source::KineticSpeed | Source::ItemNames => "Number",
        Source::KineticStress | Source::RedstonePower if progress_bar => "Progress",
        Source::KineticStress | Source::RedstonePower => "Number",
        Source::TimeOfDay | Source::StopWatch => "Instant",
        Source::NixieTube if numeric => "Number",
        Source::NixieTube => "Default",
    };
    let value = |size: i32| -> Section {
        let width = size as f32 * MONOSPACE;
        match source {
            _ if progress_bar => Section::new(width, Cycle::Pixel, false, false).wide(),
            Source::KineticSpeed | Source::KineticStress | Source::RedstonePower => {
                Section::new(width, Cycle::Numeric, false, false)
            }
            Source::NixieTube if numeric => Section::new(width, Cycle::Numeric, false, false),
            Source::TimeOfDay | Source::StopWatch => {
                Section::new(width, Cycle::Instant, false, false)
            }
            _ => Section::new(width, Cycle::Alphabet, false, false),
        }
    };
    let label = &config.label;
    if !source.allows_labeling(Target::Board) || label.is_empty() {
        if !layout.is_layout(key) {
            layout.configure(key, vec![value(max_chars)]);
        }
        return;
    }
    let len = label.chars().count() as i32;
    let name = format!("{len}_Labeled_{key}");
    if layout.is_layout(&name) {
        return;
    }
    let label_section = Section::new(
        max_chars.min(len + 1) as f32 * MONOSPACE,
        Cycle::Alphabet,
        false,
        false,
    );
    if len + 1 < max_chars {
        layout.configure(&name, vec![label_section, value(max_chars - len - 1)]);
    } else {
        layout.configure(&name, vec![label_section]);
    }
}

/// `DisplayBoardTarget.acceptFlapText` for a single-line source on a running board: from
/// `line` down, while the lines aren't reserved by another link or set by hand (`reserved`).
/// The controller's lines change; true when any section's text did.
pub fn accept_flap_text(
    board: &mut Board,
    line: usize,
    text: &[Vec<String>],
    source: Source,
    config: &Config,
    numeric: bool,
    reserved: impl Fn(usize) -> bool,
) -> bool {
    let max = board.max_chars();
    let manual = board.manual.clone();
    let lines = board.lines();
    let mut changed = false;
    let mut i = 0;
    while i + line < lines.len() {
        if i > 0 && (reserved(i + line) || manual.get(i + line).copied().unwrap_or(false)) {
            break;
        }
        // Single-line sources stop past their text.
        let Some(row) = text.get(i) else {
            break;
        };
        let layout = &mut lines[i + line];
        let before = layout.clone();
        load_flap_layout(layout, source, config, numeric, max);
        for (section, t) in layout.sections.iter_mut().zip(row) {
            section.set_text(t);
        }
        changed |= *layout != before;
        i += 1;
    }
    changed
}

/// A section on a client (`FlapDisplaySection`'s client half): the text shown and the flaps
/// still spinning toward it.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownSection {
    pub section: Section,
    pub shown: String,
    pub spinning: Vec<bool>,
    pub spinning_ticks: i32,
    pub options: usize,
}

impl ShownSection {
    /// `FlapDisplaySection.load` / `update` on a client.
    pub fn new(section: Section, options: usize) -> ShownSection {
        let n = section.flaps();
        let mut s = ShownSection {
            shown: " ".repeat(n),
            spinning: vec![false; n],
            spinning_ticks: 0,
            options,
            section,
        };
        let t = s.section.transition;
        s.refresh(t);
        s
    }

    /// `refresh(transition)`: the flaps whose character changes start spinning.
    pub fn refresh(&mut self, transition: bool) {
        let Some(text) = self.section.text.clone() else {
            return;
        };
        let n = self.spinning.len();
        let new: String = if !self.section.single_flap {
            let t = if self.section.right_aligned {
                text.trim().to_owned()
            } else {
                text
            };
            let upper: Vec<char> = t.to_uppercase().chars().take(n).collect();
            let pad = " ".repeat(n - upper.len());
            let body: String = upper.into_iter().collect();
            let new = if self.section.right_aligned {
                format!("{pad}{body}")
            } else {
                format!("{body}{pad}")
            };
            if !self.shown.is_empty() {
                for ((spin, a), b) in self
                    .spinning
                    .iter_mut()
                    .zip(self.shown.chars())
                    .zip(new.chars())
                {
                    *spin |= transition && a != b;
                }
            }
            new
        } else {
            if !self.shown.is_empty()
                && let Some(spin) = self.spinning.first_mut()
            {
                *spin |= transition && text != self.shown;
            }
            text
        };
        self.shown = new;
        self.spinning_ticks = 0;
    }

    /// `tick(instant, random)`: how many flaps turned this tick. `next_int(bound)` is the
    /// level's random.
    pub fn tick(&mut self, instant: bool, next_int: &mut impl FnMut(i32) -> i32) -> i32 {
        let max = 4.max((self.options as f32 * 1.75) as i32);
        if self.spinning_ticks > max {
            return 0;
        }
        self.spinning_ticks += 1;
        if self.spinning_ticks <= max && self.spinning_ticks < 2 {
            return if self.spinning_ticks == 1 {
                0
            } else {
                self.spinning.len() as i32
            };
        }
        let mut spinning = 0;
        let n = self.spinning.len();
        for i in 0..n {
            let chance = (8 - self.spinning_ticks).clamp(1, 10);
            let mut cont = !instant && next_int(chance * max / 4) != 0;
            cont &= max > 5 || self.spinning_ticks < 2;
            self.spinning[i] &= cont;
            if i > 0 && next_int(3) > 0 {
                self.spinning[i - 1] &= cont;
            }
            if i < n - 1 && next_int(3) > 0 {
                self.spinning[i + 1] &= cont;
            }
            if self.spinning_ticks > max {
                self.spinning[i] = false;
            }
            if self.spinning[i] {
                spinning += 1;
            }
        }
        spinning
    }

    /// No tick left to animate.
    pub fn settled(&self) -> bool {
        self.spinning_ticks > 4.max((self.options as f32 * 1.75) as i32)
    }
}

// ---------------------------------------------------------------------------------------------
// Nixie tubes

/// `DoubleFaceAttachedBlock.DoubleAttachFace`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Floor,
    Wall,
    WallReversed,
    Ceiling,
}

impl Face {
    pub fn read(v: &str) -> Option<Face> {
        Some(match v {
            "floor" => Face::Floor,
            "wall" => Face::Wall,
            "wall_reversed" => Face::WallReversed,
            "ceiling" => Face::Ceiling,
            _ => return None,
        })
    }
}

/// Every colour of nixie tube (`AllBlocks.ORANGE_NIXIE_TUBE`, `NIXIE_TUBES`).
pub fn is_nixie(b: Block) -> bool {
    matches!(
        b,
        Block::NixieTube
            | Block::WhiteNixieTube
            | Block::LightGrayNixieTube
            | Block::GrayNixieTube
            | Block::BlackNixieTube
            | Block::BrownNixieTube
            | Block::RedNixieTube
            | Block::YellowNixieTube
            | Block::LimeNixieTube
            | Block::GreenNixieTube
            | Block::CyanNixieTube
            | Block::LightBlueNixieTube
            | Block::BlueNixieTube
            | Block::PurpleNixieTube
            | Block::MagentaNixieTube
            | Block::PinkNixieTube
    )
}

/// A nixie tube's state as far as rows go (`areNixieBlocksEqual` ignores the colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NixieState {
    pub facing: Dir,
    pub face: Face,
    pub waterlogged: bool,
}

/// `NixieTubeBlock.getLeftNixieDirection`.
pub fn left_nixie(s: NixieState) -> Dir {
    match s.face {
        Face::Wall => Dir::Up,
        Face::WallReversed => Dir::Down,
        _ => s.facing.opposite(),
    }
}

/// `NixieTubeBlock.walkNixies` (no computers): the row through `start`, leftmost first; each
/// tube's place in it is its index.
pub fn walk_nixies(
    start: [i32; 3],
    nixie_at: impl Fn([i32; 3]) -> Option<NixieState>,
) -> Vec<[i32; 3]> {
    let Some(state) = nixie_at(start) else {
        return Vec::new();
    };
    let left = left_nixie(state);
    let mut cur = start;
    while nixie_at(step(cur, left, 1)) == Some(state) {
        cur = step(cur, left, 1);
    }
    let mut row = vec![cur];
    while nixie_at(step(cur, left.opposite(), 1)) == Some(state) {
        cur = step(cur, left.opposite(), 1);
        row.push(cur);
    }
    row
}

/// A nixie tube's custom text (`NixieTubeBlockEntity.customText`, `nixieIndex`); without one it
/// shows its redstone strength.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nixie {
    pub custom: Option<String>,
    pub index: i32,
    pub redstone: i32,
}

impl Nixie {
    /// `displayCustomText`: false (nothing happens) when it already shows that text.
    pub fn display_custom_text(&mut self, text: &str, index: i32) -> bool {
        if self.custom.as_deref() == Some(text) {
            return false;
        }
        self.custom = Some(text.to_owned());
        self.index = index;
        true
    }

    /// `getFullText`.
    pub fn full_text(&self) -> String {
        self.custom
            .clone()
            .unwrap_or_else(|| self.redstone.to_string())
    }

    /// `updateDisplayedStrings`: the two characters the tube shows.
    pub fn displayed(&self) -> (String, String) {
        match &self.custom {
            Some(t) => {
                let at = |i: i32| -> String {
                    t.chars()
                        .nth(i as usize)
                        .map_or_else(|| " ".to_owned(), String::from)
                };
                (at(self.index * 2), at(self.index * 2 + 1))
            }
            None => (
                if self.redstone < 10 { "0" } else { "1" }.to_owned(),
                (self.redstone % 10).to_string(),
            ),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Links

/// A display link's block entity: where it writes (`targetOffset`, `targetLine`), its source
/// and settings, and the game tick it is next due (`refreshTicks` as a deadline).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Link {
    pub target_offset: [i32; 3],
    pub target_line: i32,
    pub source: Option<Source>,
    pub config: Config,
    pub due: Option<i64>,
    /// The last reading and the text it made, kept so an unchanged reading formats nothing.
    pub last: Option<(Reading, Target, Stats, Line)>,
}

impl Link {
    /// `getTargetPosition`.
    pub fn target(&self, pos: [i32; 3]) -> [i32; 3] {
        [
            pos[0] + self.target_offset[0],
            pos[1] + self.target_offset[1],
            pos[2] + self.target_offset[2],
        ]
    }

    /// The line for a reading: the one made last time when nothing it depends on changed.
    pub fn line(&mut self, reading: Reading, target: Target, stats: Stats, lang: &Lang) -> Line {
        if let Some((r, t, s, line)) = &self.last
            && *r == reading
            && *t == target
            && *s == stats
            // The stopwatch's text follows the clock, not only its reading.
            && self.source != Some(Source::StopWatch)
        {
            return line.clone();
        }
        let Some(source) = self.source else {
            return Line {
                text: None,
                numeric: false,
            };
        };
        let line = provide_line(source, &mut self.config, &reading, target, stats, lang);
        self.last = Some((reading, target, stats, line.clone()));
        line
    }
}

/// `DisplayLinkBlockEntity.getSourcePosition`: the block it is attached to (behind `facing`).
pub fn source_pos(pos: [i32; 3], facing: Dir) -> [i32; 3] {
    step(pos, facing.opposite(), 1)
}

/// One dimension's display links and targets.
#[derive(Default)]
pub struct Displays {
    pub links: HashMap<[i32; 3], Link>,
    /// (due tick, link) for every link with a source; stale entries are skipped.
    due: BTreeSet<(i64, [i32; 3])>,
    /// Links to schedule at the next tick (just placed, loaded or given a source).
    pub fresh: Vec<[i32; 3]>,
    /// Links to refresh now (`tickSource`: a source told them, `notifyGatherers`).
    pub pulses: Vec<[i32; 3]>,
    /// Links whose neighbours changed (`powered` may follow).
    pub checks: Vec<[i32; 3]>,
    pub nixies: HashMap<[i32; 3], Nixie>,
    pub boards: HashMap<[i32; 3], Board>,
    /// Boards placed, removed or turned: their rows join again.
    pub board_checks: Vec<[i32; 3]>,
    /// `DisplayTarget.reserve`: (target, line) to the link that writes it.
    pub reserved: HashMap<([i32; 3], i32), [i32; 3]>,
    /// Nixie tubes and boards whose clients must be told again.
    pub shown: Vec<[i32; 3]>,
    /// Display positions whose clients must forget them.
    pub forgotten: Vec<[i32; 3]>,
}

impl Displays {
    /// A display block's state placed, loaded or changed (`CreateWorld::place`): links and
    /// tubes keep what was saved; a new board joins its neighbours.
    pub fn placed(&mut self, pos: [i32; 3], block: Block) {
        match block {
            // Its own `powered` changing finds it scheduled already.
            Block::DisplayLink if self.links.get(&pos).is_none_or(|l| l.due.is_none()) => {
                self.link_placed(pos)
            }
            Block::DisplayBoard if !self.boards.contains_key(&pos) => self.board_placed(pos),
            b if is_nixie(b) => self.nixie_placed(pos),
            _ => {}
        }
    }

    /// A display block gone (or turned into another block): true when it had saved state.
    pub fn removed(&mut self, pos: [i32; 3], block: Block) -> bool {
        match block {
            Block::DisplayLink => self.links.remove(&pos).is_some(),
            Block::DisplayBoard => {
                self.board_removed(pos);
                true
            }
            b if is_nixie(b) => {
                let had = self.nixies.get(&pos).is_some_and(|n| n.custom.is_some());
                self.nixie_removed(pos);
                had
            }
            _ => false,
        }
    }

    /// A link placed or loaded (its block found): kept as saved, scheduled at the next tick.
    pub fn link_placed(&mut self, pos: [i32; 3]) {
        self.links.entry(pos).or_default();
        self.fresh.push(pos);
        self.checks.push(pos);
    }

    pub fn link_removed(&mut self, pos: [i32; 3]) {
        self.links.remove(&pos);
    }

    pub fn nixie_placed(&mut self, pos: [i32; 3]) {
        self.nixies.entry(pos).or_default();
    }

    pub fn nixie_removed(&mut self, pos: [i32; 3]) {
        if self.nixies.remove(&pos).is_some() {
            self.forgotten.push(pos);
        }
    }

    pub fn board_placed(&mut self, pos: [i32; 3]) {
        self.boards.entry(pos).or_default();
        self.board_checks.push(pos);
    }

    pub fn board_removed(&mut self, pos: [i32; 3]) {
        if self.boards.remove(&pos).is_some() {
            self.forgotten.push(pos);
        }
        self.board_checks.push(pos);
    }

    /// Schedules a link `refresh_ticks` from `now` (`refreshTicks = 0`); none without a source.
    pub fn schedule(&mut self, pos: [i32; 3], now: i64) {
        let Some(link) = self.links.get_mut(&pos) else {
            return;
        };
        link.due = link.source.map(|s| now + s.refresh_ticks());
        if let Some(t) = link.due {
            self.due.insert((t, pos));
        }
    }

    /// The links due by `now` (`tick`: `refreshTicks` reached the source's interval), the fresh
    /// ones scheduled first. Nothing is looked at for links not due.
    pub fn take_due(&mut self, now: i64) -> Vec<[i32; 3]> {
        for pos in std::mem::take(&mut self.fresh) {
            if self.links.get(&pos).is_some_and(|l| l.due.is_none()) {
                self.schedule(pos, now);
            }
        }
        let mut out = Vec::new();
        while let Some(&(t, pos)) = self.due.first() {
            if t > now {
                break;
            }
            self.due.pop_first();
            if self.links.get(&pos).is_some_and(|l| l.due == Some(t)) {
                out.push(pos);
            }
        }
        out
    }

    /// Whether anything waits for this tick (the cheap test before the glue looks further).
    pub fn idle(&self, now: i64) -> bool {
        self.fresh.is_empty()
            && self.pulses.is_empty()
            && self.checks.is_empty()
            && self.board_checks.is_empty()
            && self.shown.is_empty()
            && self.forgotten.is_empty()
            && self.due.first().is_none_or(|&(t, _)| t > now)
    }

    /// `DisplayLinkBlock.notifyGatherers`: links attached to `pos` with a source refresh.
    pub fn notify_gatherers(&mut self, pos: [i32; 3], facing_of: impl Fn([i32; 3]) -> Option<Dir>) {
        for d in Dir::ALL {
            let at = step(pos, d, 1);
            let Some(facing) = facing_of(at) else {
                continue;
            };
            // `getDirection() == d.getOpposite()`: it is attached to `pos`.
            if facing != d {
                continue;
            }
            if self.links.get(&at).is_some_and(|l| l.source.is_some()) {
                self.pulses.push(at);
            }
        }
    }

    /// `DisplayTarget.isReserved` without the clean-up: another link that is still a link
    /// (`is_link`) writes that line.
    pub fn is_reserved(
        &mut self,
        target: [i32; 3],
        line: i32,
        link: [i32; 3],
        is_link: impl Fn([i32; 3]) -> bool,
    ) -> bool {
        match self.reserved.get(&(target, line)) {
            None => false,
            Some(&by) if by != link && is_link(by) => true,
            Some(_) => {
                self.reserved.remove(&(target, line));
                false
            }
        }
    }

    /// `DisplayTarget.reserve` (line 0 is never reserved).
    pub fn reserve(&mut self, target: [i32; 3], line: i32, link: [i32; 3]) {
        if line != 0 {
            self.reserved.insert((target, line), link);
        }
    }

    /// Forgets a column (`chunk_unload`).
    pub fn column_unloaded(&mut self, cx: i32, cz: i32) {
        let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
        self.links.retain(|p, _| !inside(p));
        self.nixies.retain(|p, _| !inside(p));
        self.boards.retain(|p, _| !inside(p));
        self.reserved.retain(|(p, _), _| !inside(p));
        self.due.retain(|(_, p)| !inside(p));
        for list in [
            &mut self.fresh,
            &mut self.pulses,
            &mut self.checks,
            &mut self.board_checks,
            &mut self.shown,
            &mut self.forgotten,
        ] {
            list.retain(|p| !inside(p));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Messages

/// What a client is told of a nixie tube or a board.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    /// A tube's custom text and place in its row (None: its redstone strength).
    Nixie {
        pos: [i32; 3],
        text: Option<String>,
        index: i32,
        redstone: i32,
    },
    /// A controller's size and lines (`FlapDisplayBlockEntity.write` for a client).
    Board {
        pos: [i32; 3],
        x_size: i32,
        y_size: i32,
        lines: Vec<Layout>,
        colour: Vec<Option<u8>>,
        glowing: Vec<bool>,
    },
    Forget {
        pos: [i32; 3],
    },
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = &s.as_bytes()[..s.len().min(u16::MAX as usize)];
    out.extend_from_slice(&(b.len() as u16).to_le_bytes());
    out.extend_from_slice(b);
}

fn put_pos(out: &mut Vec<u8>, pos: [i32; 3]) {
    for v in pos {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

pub fn encode(shown: &[Shown]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in shown {
        match s {
            Shown::Nixie {
                pos,
                text,
                index,
                redstone,
            } => {
                out.push(0);
                put_pos(&mut out, *pos);
                out.extend_from_slice(&index.to_le_bytes());
                out.extend_from_slice(&redstone.to_le_bytes());
                out.push(u8::from(text.is_some()));
                put_str(&mut out, text.as_deref().unwrap_or(""));
            }
            Shown::Board {
                pos,
                x_size,
                y_size,
                lines,
                colour,
                glowing,
            } => {
                out.push(1);
                put_pos(&mut out, *pos);
                out.extend_from_slice(&x_size.to_le_bytes());
                out.extend_from_slice(&y_size.to_le_bytes());
                out.push(lines.len() as u8);
                for (i, l) in lines.iter().enumerate() {
                    out.push(colour.get(i).copied().flatten().unwrap_or(255));
                    out.push(u8::from(glowing.get(i).copied().unwrap_or(false)));
                    put_str(&mut out, &l.key);
                    out.push(l.sections.len() as u8);
                    for s in &l.sections {
                        out.extend_from_slice(&s.size.to_le_bytes());
                        out.push(s.cycle as u8);
                        out.push(
                            u8::from(s.single_flap)
                                | u8::from(s.has_gap) << 1
                                | u8::from(s.right_aligned) << 2
                                | u8::from(s.wide) << 3
                                | u8::from(s.transition) << 4
                                | u8::from(s.text.is_some()) << 5,
                        );
                        put_str(&mut out, s.text.as_deref().unwrap_or(""));
                    }
                }
            }
            Shown::Forget { pos } => {
                out.push(2);
                put_pos(&mut out, *pos);
            }
        }
    }
    out
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn pos(&mut self) -> Option<[i32; 3]> {
        Some([self.i32()?, self.i32()?, self.i32()?])
    }
    fn str(&mut self) -> Option<String> {
        let n = u16::from_le_bytes(self.take(2)?.try_into().ok()?);
        Some(String::from_utf8_lossy(self.take(usize::from(n))?).into_owned())
    }
}

pub fn decode(data: &[u8]) -> Vec<Shown> {
    let mut r = Reader(data);
    let mut out = Vec::new();
    let one = |r: &mut Reader| -> Option<Shown> {
        Some(match r.u8()? {
            0 => {
                let pos = r.pos()?;
                let index = r.i32()?;
                let redstone = r.i32()?;
                let has = r.u8()? != 0;
                let text = r.str()?;
                Shown::Nixie {
                    pos,
                    text: has.then_some(text),
                    index,
                    redstone,
                }
            }
            1 => {
                let pos = r.pos()?;
                let x_size = r.i32()?;
                let y_size = r.i32()?;
                let n = r.u8()?;
                let (mut lines, mut colour, mut glowing) = (Vec::new(), Vec::new(), Vec::new());
                for _ in 0..n {
                    let c = r.u8()?;
                    colour.push((c != 255).then_some(c));
                    glowing.push(r.u8()? != 0);
                    let key = r.str()?;
                    let m = r.u8()?;
                    let mut sections = Vec::new();
                    for _ in 0..m {
                        let size = r.f32()?;
                        let cycle = *Cycle::ALL.get(usize::from(r.u8()?))?;
                        let f = r.u8()?;
                        let text = r.str()?;
                        sections.push(Section {
                            size,
                            cycle,
                            single_flap: f & 1 != 0,
                            has_gap: f & 2 != 0,
                            right_aligned: f & 4 != 0,
                            wide: f & 8 != 0,
                            transition: f & 16 != 0,
                            text: (f & 32 != 0).then_some(text),
                        });
                    }
                    lines.push(Layout { key, sections });
                }
                Shown::Board {
                    pos,
                    x_size,
                    y_size,
                    lines,
                    colour,
                    glowing,
                }
            }
            2 => Shown::Forget { pos: r.pos()? },
            _ => return None,
        })
    };
    while !r.0.is_empty() {
        match one(&mut r) {
            Some(s) => out.push(s),
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: Stats = Stats {
        rows: 2,
        columns: 9,
    };
    const NIXIE: Stats = Stats {
        rows: 1,
        columns: 4,
    };
    const SIGN: Stats = Stats {
        rows: 4,
        columns: 15,
    };

    fn line(
        source: Source,
        config: &mut Config,
        reading: Reading,
        target: Target,
        stats: Stats,
    ) -> Option<String> {
        provide_line(source, config, &reading, target, stats, &Lang::EN_US).text
    }

    #[test]
    fn speed_is_formatted_as_create_does() {
        let mut c = Config::default();
        let speed = |c: &mut Config, s: f32| {
            line(
                Source::KineticSpeed,
                c,
                Reading::Speed(Some(s)),
                Target::Nixie,
                NIXIE,
            )
        };
        // CreateLang.number(speed).space().translate("generic.unit.rpm"), absolute by default.
        assert_eq!(speed(&mut c, -16.0).as_deref(), Some("16 RPM"));
        assert_eq!(speed(&mut c, 1234.5).as_deref(), Some("1,234.5 RPM"));
        assert_eq!(speed(&mut c, 0.333).as_deref(), Some("0.33 RPM"));
        c.directional = 1;
        assert_eq!(speed(&mut c, -16.0).as_deref(), Some("-16 RPM"));
        assert_eq!(speed(&mut c, -0.000001).as_deref(), Some("0 RPM"));
        // Not a speedometer: ZERO.
        assert_eq!(
            line(
                Source::KineticSpeed,
                &mut c,
                Reading::Speed(None),
                Target::Nixie,
                NIXIE
            )
            .as_deref(),
            Some("0")
        );
        // The label, with a space, before the line.
        c.label = "Speed".into();
        let l = provide_line(
            Source::KineticSpeed,
            &mut c,
            &Reading::Speed(Some(32.0)),
            Target::Sign { hanging: false },
            SIGN,
            &Lang::EN_US,
        );
        assert_eq!(
            provide_text(
                Source::KineticSpeed,
                &c,
                &l,
                Target::Sign { hanging: false }
            ),
            vec!["Speed 32 RPM".to_string()]
        );
        assert_eq!(
            provide_flap_text(Source::KineticSpeed, &c, &l),
            vec![vec!["Speed ".to_string(), "32 RPM".to_string()]]
        );
    }

    #[test]
    fn stress_modes_and_bars() {
        let mut c = Config::default();
        let r = Reading::Stress(Some((256.0, 1024.0)));
        // Mode 0: a bar as wide as the target's columns, on a sign 6/9 of them (10, 2 filled).
        assert_eq!(
            line(
                Source::KineticStress,
                &mut c,
                r.clone(),
                Target::Sign { hanging: false },
                SIGN
            )
            .as_deref(),
            Some(
                "\u{2588}\u{2588}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}"
            )
        );
        // On a nixie row of 4 characters: one of four filled.
        assert_eq!(
            line(
                Source::KineticStress,
                &mut c,
                r.clone(),
                Target::Nixie,
                NIXIE
            )
            .as_deref(),
            Some("\u{2588}\u{2592}\u{2592}\u{2592}")
        );
        // On a board of 9 columns: 7 wide characters (9 * 7 / 9).
        assert_eq!(
            line(
                Source::KineticStress,
                &mut c,
                r.clone(),
                Target::Board,
                BOARD
            )
            .map(|s| s.chars().count()),
            Some(7)
        );
        c.mode = 1;
        assert_eq!(
            line(
                Source::KineticStress,
                &mut c,
                r.clone(),
                Target::Nixie,
                NIXIE
            )
            .as_deref(),
            Some("25%")
        );
        c.mode = 2;
        assert_eq!(
            line(
                Source::KineticStress,
                &mut c,
                r.clone(),
                Target::Nixie,
                NIXIE
            )
            .as_deref(),
            Some("256su")
        );
        assert_eq!(
            line(
                Source::KineticStress,
                &mut c,
                r.clone(),
                Target::Board,
                BOARD
            )
            .as_deref(),
            Some("256 su")
        );
        c.mode = 3;
        assert_eq!(
            line(
                Source::KineticStress,
                &mut c,
                r.clone(),
                Target::Nixie,
                NIXIE
            )
            .as_deref(),
            Some("1,024su")
        );
        c.mode = 4;
        assert_eq!(
            line(
                Source::KineticStress,
                &mut c,
                r.clone(),
                Target::Nixie,
                NIXIE
            )
            .as_deref(),
            Some("768su")
        );
        // No capacity: 0 whatever the mode.
        assert_eq!(
            line(
                Source::KineticStress,
                &mut c,
                Reading::Stress(Some((5.0, 0.0))),
                Target::Nixie,
                NIXIE
            )
            .as_deref(),
            Some("0su")
        );
        // Not a stressometer: EMPTY_LINE, an empty text.
        let l = provide_line(
            Source::KineticStress,
            &mut c,
            &Reading::Stress(None),
            Target::Nixie,
            NIXIE,
            &Lang::EN_US,
        );
        assert_eq!(
            provide_text(Source::KineticStress, &c, &l, Target::Nixie),
            vec![String::new()]
        );
    }

    #[test]
    fn redstone_power_number_or_bar() {
        let mut c = Config::default();
        assert_eq!(
            line(
                Source::RedstonePower,
                &mut c,
                Reading::Power(7),
                Target::Nixie,
                NIXIE
            )
            .as_deref(),
            Some("7")
        );
        c.mode = 1;
        c.label = "P".into();
        // 15 columns less the label and its space, 6/9 of that on a sign: 8, half filled at 8/15.
        assert_eq!(
            line(
                Source::RedstonePower,
                &mut c,
                Reading::Power(8),
                Target::Sign { hanging: false },
                SIGN
            )
            .as_deref(),
            Some("\u{2588}\u{2588}\u{2588}\u{2588}\u{2592}\u{2592}\u{2592}\u{2592}")
        );
    }

    #[test]
    fn time_of_day_and_stopwatch() {
        let lang = Lang::EN_US;
        // Day time 0 is 6:00 am; minutes in fives.
        assert_eq!(time_of_day(0, true, true, [0, 0], &lang), " 6:00 am");
        assert_eq!(time_of_day(0, true, false, [0, 0], &lang), " 6:00");
        assert_eq!(
            time_of_day(18000 + 24000 * 3, true, true, [0, 0], &lang),
            "12:00 am"
        );
        assert_eq!(time_of_day(6000, true, true, [0, 0], &lang), "12:00 pm");
        assert_eq!(time_of_day(13_570, true, false, [0, 0], &lang), "19:30");
        // A dimension without a day: random hours 24..93 and minutes 60..99.
        assert_eq!(time_of_day(0, false, false, [10, 5], &lang), "34:65");
        // A clock that does not turn shows EMPTY_TIME.
        let mut c = Config::default();
        let r = Reading::Time {
            day_time: 0,
            natural: true,
            running: false,
            random: [0, 0],
        };
        assert_eq!(
            line(Source::TimeOfDay, &mut c, r, Target::Nixie, NIXIE).as_deref(),
            Some(EMPTY_TIME)
        );
        // Stopwatch: started at its first reading.
        assert_eq!(stopwatch(0), " 0:00");
        assert_eq!(stopwatch(20 * 65), " 1:05");
        assert_eq!(stopwatch(20 * 60 * 12 + 20 * 3), "12:03");
        assert_eq!(stopwatch(20 * 3600 * 2 + 20 * 61), " 2:01:01");
        let r = |now| Reading::Watch { now, running: true };
        assert_eq!(
            line(Source::StopWatch, &mut c, r(100), Target::Nixie, NIXIE).as_deref(),
            Some(" 0:00")
        );
        assert_eq!(c.start_time, Some(100));
        assert_eq!(
            line(
                Source::StopWatch,
                &mut c,
                r(100 + 20 * 75),
                Target::Nixie,
                NIXIE
            )
            .as_deref(),
            Some(" 1:15")
        );
        assert_eq!(Source::StopWatch.refresh_ticks(), 20);
        assert_eq!(Source::TimeOfDay.refresh_ticks(), 100);
    }

    #[test]
    fn item_names_and_nixie_copies() {
        let mut c = Config::default();
        let r = Reading::Names(vec!["Iron Ingot".into(), "Dirt".into()]);
        assert_eq!(
            line(Source::ItemNames, &mut c, r, Target::Nixie, NIXIE).as_deref(),
            Some("Iron IngotDirt")
        );
        // No item: an empty line, not EMPTY_LINE.
        assert_eq!(
            line(
                Source::ItemNames,
                &mut c,
                Reading::Names(vec![]),
                Target::Nixie,
                NIXIE
            )
            .as_deref(),
            Some("")
        );
        let l = provide_line(
            Source::NixieTube,
            &mut c,
            &Reading::Nixie(Some("12".into())),
            Target::Board,
            BOARD,
            &Lang::EN_US,
        );
        assert!(l.numeric);
        let l = provide_line(
            Source::NixieTube,
            &mut c,
            &Reading::Nixie(Some("1a".into())),
            Target::Board,
            BOARD,
            &Lang::EN_US,
        );
        assert!(!l.numeric);
        // Copied onto other nixie tubes, the label is left out.
        c.label = "L".into();
        assert_eq!(
            provide_text(Source::NixieTube, &c, &l, Target::Nixie),
            vec!["1a".to_string()]
        );
        assert_eq!(
            provide_text(Source::NixieTube, &c, &l, Target::Board),
            vec!["L 1a".to_string()]
        );
    }

    #[test]
    fn links_refresh_only_when_due() {
        let mut d = Displays::default();
        let pos = [0, 64, 0];
        d.link_placed(pos);
        d.checks.clear();
        // No source: never due.
        assert!(d.take_due(0).is_empty());
        assert!(d.take_due(1000).is_empty());
        d.links.get_mut(&pos).unwrap().source = Some(Source::KineticSpeed);
        d.links.get_mut(&pos).unwrap().due = None;
        d.fresh.push(pos);
        // Scheduled 100 ticks on: nothing before, then once.
        assert!(d.take_due(10).is_empty());
        assert!(d.idle(109));
        assert!(!d.idle(110));
        assert!(d.take_due(109).is_empty());
        assert_eq!(d.take_due(110), vec![pos]);
        assert!(
            d.take_due(500).is_empty(),
            "taken once until scheduled again"
        );
        d.schedule(pos, 110);
        assert!(d.idle(209));
        assert_eq!(d.take_due(210), vec![pos]);
        // A pulse rescheduling it makes the old deadline stale.
        d.schedule(pos, 210);
        d.schedule(pos, 250);
        assert!(d.take_due(310).is_empty());
        assert_eq!(d.take_due(350), vec![pos]);
        // The stopwatch every 20.
        d.links.get_mut(&pos).unwrap().source = Some(Source::StopWatch);
        d.schedule(pos, 350);
        assert_eq!(d.take_due(370), vec![pos]);
        // Removed: never again.
        d.schedule(pos, 370);
        d.link_removed(pos);
        assert!(d.take_due(10_000).is_empty());
    }

    #[test]
    fn an_unchanged_reading_formats_nothing_again() {
        let mut l = Link {
            source: Some(Source::KineticSpeed),
            ..Default::default()
        };
        let a = l.line(
            Reading::Speed(Some(8.0)),
            Target::Nixie,
            NIXIE,
            &Lang::EN_US,
        );
        assert_eq!(a.text.as_deref(), Some("8 RPM"));
        assert!(l.last.is_some());
        let b = l.line(
            Reading::Speed(Some(8.0)),
            Target::Nixie,
            NIXIE,
            &Lang::EN_US,
        );
        assert_eq!(a, b);
        let c = l.line(
            Reading::Speed(Some(16.0)),
            Target::Nixie,
            NIXIE,
            &Lang::EN_US,
        );
        assert_eq!(c.text.as_deref(), Some("16 RPM"));
    }

    #[test]
    fn nixie_text_spreads_two_characters_a_tube() {
        // A row of three tubes on the floor facing north: left is south.
        let s = NixieState {
            facing: Dir::North,
            face: Face::Floor,
            waterlogged: false,
        };
        let tubes = [[0, 0, 0], [0, 0, 1], [0, 0, 2]];
        let at = |p: [i32; 3]| tubes.contains(&p).then_some(s);
        assert_eq!(left_nixie(s), Dir::South);
        // From the middle, the row starts at its left end (most south).
        let row = walk_nixies([0, 0, 1], at);
        assert_eq!(row, vec![[0, 0, 2], [0, 0, 1], [0, 0, 0]]);
        // Another facing breaks the row.
        let other = |p: [i32; 3]| match p {
            [0, 0, 2] => Some(NixieState {
                facing: Dir::South,
                ..s
            }),
            p if tubes.contains(&p) => Some(s),
            _ => None,
        };
        assert_eq!(walk_nixies([0, 0, 0], other), vec![[0, 0, 1], [0, 0, 0]]);
        // On a wall, left is up.
        assert_eq!(
            left_nixie(NixieState {
                face: Face::Wall,
                ..s
            }),
            Dir::Up
        );
        assert_eq!(
            left_nixie(NixieState {
                face: Face::WallReversed,
                ..s
            }),
            Dir::Down
        );
        // "16 RPM" across the row: each tube shows its two characters, spaces past the end.
        let mut tubes: Vec<Nixie> = vec![Nixie::default(); 3];
        for (i, t) in tubes.iter_mut().enumerate() {
            assert!(t.display_custom_text("16 RPM!", i as i32));
        }
        let shown: Vec<(String, String)> = tubes.iter().map(Nixie::displayed).collect();
        assert_eq!(
            shown,
            vec![
                ("1".into(), "6".into()),
                (" ".into(), "R".into()),
                ("P".into(), "M".into())
            ]
        );
        let mut t = Nixie {
            custom: Some("ab".into()),
            index: 0,
            redstone: 0,
        };
        assert!(
            !t.display_custom_text("ab", 3),
            "the same text changes nothing"
        );
        assert_eq!(t.index, 0);
        assert_eq!(t.displayed(), ("a".into(), "b".into()));
        // Past the text: spaces.
        let t = Nixie {
            custom: Some("abc".into()),
            index: 1,
            redstone: 0,
        };
        assert_eq!(t.displayed(), ("c".into(), " ".into()));
        // Without custom text, its redstone strength.
        let t = Nixie {
            custom: None,
            index: 0,
            redstone: 12,
        };
        assert_eq!(t.displayed(), ("1".into(), "2".into()));
        assert_eq!(t.full_text(), "12");
    }

    #[test]
    fn display_board_layout_and_controller() {
        // A wall of boards facing south, 3 wide (x 0..3) and 2 high (y 0..2).
        let facing = Dir::South;
        let boards: Vec<[i32; 3]> = (0..3)
            .flat_map(|x| (0..2).map(move |y| [x, y, 0]))
            .collect();
        let facing_at = |p: [i32; 3]| boards.contains(&p).then_some(facing);
        for &p in &boards {
            let (down, up) = column_connections(p, facing, facing_at);
            assert_eq!((down, up), (p[1] == 1, p[1] == 0), "{p:?}");
        }
        let state_at = |p: [i32; 3]| {
            boards.contains(&p).then(|| BoardState {
                facing,
                up: p[1] == 0,
                down: p[1] == 1,
                waterlogged: false,
            })
        };
        // Clockwise of south is west: the controller is the top board at the east end... its
        // left (west) neighbour differs only where none is; Create's own rule.
        let left = clockwise(facing);
        assert_eq!(left, Dir::West);
        let top_west = [0, 1, 0];
        let (c, x, y) = controller_status(top_west, state_at(top_west).unwrap(), state_at);
        assert_eq!((c, x, y), (true, 3, 2));
        let (c, _, _) = controller_status([1, 1, 0], state_at([1, 1, 0]).unwrap(), state_at);
        assert!(!c);
        let (c, _, _) = controller_status([0, 0, 0], state_at([0, 0, 0]).unwrap(), state_at);
        assert!(!c, "a board joined above is never a controller");
        // Every board finds it.
        for &p in &boards {
            assert_eq!(controller_pos(p, facing, state_at), Some(top_west), "{p:?}");
        }
        // A row with a gap below one board does not join down.
        let ragged: Vec<[i32; 3]> = vec![[0, 1, 0], [1, 1, 0], [0, 0, 0]];
        let ragged_at = |p: [i32; 3]| ragged.contains(&p).then_some(facing);
        assert_eq!(
            column_connections([0, 1, 0], facing, ragged_at),
            (false, false)
        );
        assert_eq!(
            column_connections([0, 0, 0], facing, ragged_at),
            (false, false)
        );

        // Three boards wide: (48 - 2) / 3.5 = 13 characters; two lines a board high.
        let mut b = Board::default();
        assert!(b.set_status(true, 3, 2));
        assert_eq!(b.max_chars(), 13);
        assert_eq!(b.max_chars_with(1), 12);
        assert_eq!(
            b.stats(),
            Stats {
                rows: 4,
                columns: 13
            }
        );
        assert_eq!(b.lines().len(), 4);
        assert!(!b.set_status(true, 3, 2));

        // Speed labelled "Mix": a label section of 4 and a numeric section of the rest.
        let config = Config {
            label: "Mix".into(),
            ..Default::default()
        };
        let text = provide_flap_text(
            Source::KineticSpeed,
            &config,
            &Line {
                text: Some("64 RPM".into()),
                numeric: false,
            },
        );
        assert!(accept_flap_text(
            &mut b,
            1,
            &text,
            Source::KineticSpeed,
            &config,
            false,
            |_| false
        ));
        let line = &b.lines()[1];
        assert_eq!(line.key, "3_Labeled_Number");
        assert_eq!(line.sections.len(), 2);
        assert_eq!(line.sections[0].size, 4.0 * MONOSPACE);
        assert_eq!(line.sections[0].cycle, Cycle::Alphabet);
        assert_eq!(line.sections[1].size, 9.0 * MONOSPACE);
        assert_eq!(line.sections[1].cycle, Cycle::Numeric);
        assert_eq!(line.sections[1].text.as_deref(), Some("64 RPM"));
        assert!(line.sections[1].transition);
        // The others keep their default layout.
        assert_eq!(b.lines()[0].key, "Default");
        // The same text again: the layout is the same, only `sendTransition` is set again.
        for s in &mut b.lines()[1].sections {
            s.transition = false;
        }
        assert!(accept_flap_text(
            &mut b,
            1,
            &text,
            Source::KineticSpeed,
            &config,
            false,
            |_| false
        ));
        // Unlabelled progress: wide pixel flaps.
        let mut l = Layout::new(13);
        load_flap_layout(&mut l, Source::KineticStress, &Config::default(), false, 13);
        assert_eq!(l.key, "Progress");
        assert!(l.sections[0].wide);
        assert_eq!(l.sections[0].cycle, Cycle::Pixel);
        // A label too long for the line keeps only the label.
        let long = Config {
            label: "A very long label".into(),
            ..Default::default()
        };
        load_flap_layout(&mut l, Source::TimeOfDay, &long, false, 13);
        assert_eq!(l.key, "17_Labeled_Instant");
        assert_eq!(l.sections.len(), 1);
        assert_eq!(l.sections[0].size, 13.0 * MONOSPACE);
    }

    #[test]
    fn flaps_spin_to_new_text_then_settle() {
        let mut s = Section::new(4.0 * MONOSPACE, Cycle::Numeric, false, false);
        s.text = Some("12".into());
        let mut shown = ShownSection::new(s.clone(), 11);
        assert_eq!(shown.shown, "12  ");
        // Loaded with a transition: every changed flap spins.
        s.set_text("13");
        let mut shown2 = ShownSection {
            section: s.clone(),
            ..shown.clone()
        };
        shown2.refresh(true);
        assert_eq!(shown2.shown, "13  ");
        assert_eq!(shown2.spinning, vec![false, true, false, false]);
        // Lower case goes up, text past the flaps is cut.
        shown.section.text = Some("abcdef".into());
        shown.refresh(false);
        assert_eq!(shown.shown, "ABCD");
        assert_eq!(shown.spinning, vec![false; 4]);
        // Ticks: the first turns nothing, then fewer flaps each tick, then none.
        let mut seed = 7u32;
        let mut next = |bound: i32| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            ((seed >> 16) as i32).rem_euclid(bound.max(1))
        };
        assert_eq!(shown2.tick(false, &mut next), 0);
        let mut ticks = 1;
        while !shown2.settled() {
            shown2.tick(false, &mut next);
            ticks += 1;
        }
        // max(4, 11 * 1.75) = 19 ticks of spinning.
        assert_eq!(ticks, 20);
        assert_eq!(shown2.spinning, vec![false; 4]);
        // Instant (over 128 rpm): every flap stops at the second tick.
        let mut s3 = shown.clone();
        s3.spinning = vec![true; 4];
        s3.spinning_ticks = 0;
        assert_eq!(s3.tick(true, &mut next), 0);
        assert_eq!(s3.tick(true, &mut next), 0);
        assert_eq!(s3.spinning, vec![false; 4]);
    }

    #[test]
    fn messages_round_trip() {
        let mut b = Board::default();
        b.set_status(true, 2, 1);
        b.lines()[0].sections[0].set_text("HI");
        let shown = vec![
            Shown::Nixie {
                pos: [1, 2, 3],
                text: Some("16 RPM".into()),
                index: 2,
                redstone: 0,
            },
            Shown::Nixie {
                pos: [1, 2, 4],
                text: None,
                index: 0,
                redstone: 9,
            },
            Shown::Board {
                pos: [-5, 70, 9],
                x_size: 2,
                y_size: 1,
                lines: b.lines().clone(),
                colour: vec![Some(3), None],
                glowing: vec![false, true],
            },
            Shown::Forget { pos: [0, 0, 0] },
        ];
        assert_eq!(decode(&encode(&shown)), shown);
    }

    #[test]
    fn sources_by_id_and_legacy_name() {
        for s in Source::ALL {
            assert_eq!(Source::from_id(s.id()), Some(s));
            assert_eq!(Source::from_id(&format!("create:{}", s.id())), Some(s));
        }
        assert_eq!(
            Source::from_id("speedometer_source_kinetic_speed"),
            Some(Source::KineticSpeed)
        );
        assert_eq!(Source::from_id("nope"), None);
        let mut c = Config::default();
        assert!(c.set("Label", "Hi there"));
        assert!(c.set("Mode", "2"));
        assert!(!c.set("Other", "1"));
        assert_eq!((c.label.as_str(), c.mode), ("Hi there", 2));
    }

    #[test]
    fn reserved_lines_stop_a_board_and_gatherers_hear_their_source() {
        let mut d = Displays::default();
        let (board, a, b) = ([0, 0, 0], [5, 0, 0], [6, 0, 0]);
        // Line 0 is never reserved; line 1 by link `b` while it is a link.
        d.reserve(board, 0, a);
        assert!(d.reserved.is_empty());
        d.reserve(board, 1, b);
        assert!(d.is_reserved(board, 1, a, |_| true));
        assert!(!d.is_reserved(board, 1, b, |_| true), "not for itself");
        assert!(
            !d.is_reserved(board, 1, a, |_| false),
            "a link gone frees it"
        );
        assert!(d.reserved.is_empty());
        // A multi-row text written from line 0 stops at a reserved line.
        let mut bd = Board::default();
        bd.set_status(true, 2, 2);
        let text = vec![vec!["A".to_string()], vec!["B".to_string()]];
        accept_flap_text(
            &mut bd,
            0,
            &text,
            Source::ItemNames,
            &Config::default(),
            false,
            |l| l == 1,
        );
        assert_eq!(bd.lines()[0].sections[0].text.as_deref(), Some("A"));
        assert_eq!(bd.lines()[1].sections[0].text, None);
        // Lines set by hand are kept too.
        bd.manual[1] = true;
        accept_flap_text(
            &mut bd,
            0,
            &text,
            Source::ItemNames,
            &Config::default(),
            false,
            |_| false,
        );
        assert_eq!(bd.lines()[1].sections[0].text, None);
        // A tube that changes tells the links attached to it (and with a source) to refresh.
        let tube = [0, 64, 0];
        for (p, s) in [
            ([0, 65, 0], Some(Source::NixieTube)),
            ([1, 64, 0], None),
            ([0, 64, 1], Some(Source::NixieTube)),
        ] {
            d.links.insert(
                p,
                Link {
                    source: s,
                    ..Default::default()
                },
            );
        }
        let facing = |p: [i32; 3]| match p {
            [0, 65, 0] => Some(Dir::Up),
            [1, 64, 0] => Some(Dir::East),
            [0, 64, 1] => Some(Dir::North),
            _ => None,
        };
        d.notify_gatherers(tube, facing);
        assert_eq!(
            d.pulses,
            vec![[0, 65, 0]],
            "only the one on it with a source"
        );
        // Placed and removed by block.
        d.placed([9, 9, 9], Block::DisplayLink);
        assert!(d.links.contains_key(&[9, 9, 9]) && d.fresh.contains(&[9, 9, 9]));
        d.placed([8, 8, 8], Block::NixieTube);
        assert!(d.nixies.contains_key(&[8, 8, 8]));
        assert!(d.removed([9, 9, 9], Block::DisplayLink));
        assert!(
            !d.removed([8, 8, 8], Block::NixieTube),
            "no custom text to save"
        );
        assert_eq!(d.forgotten, vec![[8, 8, 8]]);
    }
}
