//! The train schedule item (`ScheduleItem`, `Schedule`, `ScheduleEntry`, the instructions of
//! `destination/` and the conditions of `condition/`, `ScheduleEditPacket`,
//! `ScheduleItemEntityInteraction`): its schedule kept in the stack's custom data under
//! Create's component name (`create:train_schedule`) in Create's own NBT layout, the screen it
//! opens (`schedule_screen`), its tooltip, and handing it to a train's conductor or taking it
//! back.
//!
//! A train runs what the engine's schedule can (`create_engine::schedule`): travelling to a
//! station and waiting for a delay or for idle cargo. A schedule handed over keeps everything
//! the screen wrote (the train is saved with it and gives it back as it was), but:
//! - entries of the other instructions (rename, throttle, package delivery and retrieval) are
//!   left out of what the train runs;
//! - the other conditions (time of day, item and fluid cargo, redstone link, players seated,
//!   station powered, chunk unloaded) are never met, so a group holding one waits on its
//!   other groups.
//!
//! Left out (no API for it):
//! - Only players sit on a carriage's seats here (`trains::riding`), so a conductor is a
//!   player in a conductor's seat; Create's mobs and blaze burners as conductors have no seat.
//! - The client can't tell a carriage's seat from a world seat (both are item displays it
//!   can't read the tags of), so a schedule used on anyone seated never opens its screen.
//! - The 5-tick item cooldown (`getCooldowns().addCooldown`): as in the legacy glue.

use super::super::net;
use super::schedule_screen::{self, Opened};
use super::{action_bar, custom, give_back, held, set_custom, set_held, sneaking};
use mod_api::{
    Compound, EntityInteractEvent, Hand, Host, HostSide, InteractionResult, ItemPart, ItemStack,
    PlayerRef, ServerHost, SoundSource, Tag, Text, TooltipEvent, UseItemEvent, UseOnEvent,
};

/// The schedule item.
pub(crate) const ITEM: &str = "create:schedule";
/// `AllDataComponents.TRAIN_SCHEDULE`, kept in the stack's custom data.
pub(crate) const KEY: &str = "create:train_schedule";

/// `Schedule.INSTRUCTION_TYPES`, in Create's order (the editor's choices).
pub(crate) const INSTRUCTIONS: [&str; 5] = [
    "destination",
    "package_delivery",
    "package_retrieval",
    "rename",
    "throttle",
];
/// `Schedule.CONDITION_TYPES`, in Create's order.
pub(crate) const CONDITIONS: [&str; 9] = [
    "delay",
    "time_of_day",
    "fluid_threshold",
    "item_threshold",
    "redstone_link",
    "player_count",
    "idle",
    "unloaded",
    "powered",
];

/// An instruction or a condition (`ScheduleDataEntry`): its type (the path of its id, all of
/// Create's) and its `data` compound, which the screen's widgets read and write by key.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Input {
    pub id: String,
    pub data: Compound,
}

impl Input {
    /// A new one of type `id`, with its constructor's defaults.
    pub(crate) fn new(id: &str) -> Input {
        let mut i = Input {
            id: id.to_owned(),
            data: Compound::new(),
        };
        match id {
            "throttle" => i.set_int("Value", 100),
            "delay" | "idle" => {
                i.set_int("Value", 5);
                i.set_int("TimeUnit", 1);
            }
            "time_of_day" => {
                i.set_int("Hour", 8);
                i.set_int("Rotation", 5);
            }
            "item_threshold" | "fluid_threshold" => i.set_text("Threshold", "10"),
            _ => {}
        }
        i
    }

    /// `intData`: 0 when missing.
    pub(crate) fn int(&self, key: &str) -> i32 {
        match self.data.get(&key.into()) {
            Some(Tag::Int(v)) => *v,
            Some(Tag::Byte(v)) => i32::from(*v),
            Some(Tag::Short(v)) => i32::from(*v),
            Some(Tag::Long(v)) => *v as i32,
            _ => 0,
        }
    }

    pub(crate) fn set_int(&mut self, key: &str, v: i32) {
        self.data.insert(key.into(), Tag::Int(v));
    }

    /// `textData`: "" when missing.
    pub(crate) fn text(&self, key: &str) -> String {
        match self.data.get(&key.into()) {
            Some(Tag::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        }
    }

    pub(crate) fn set_text(&mut self, key: &str, v: &str) {
        self.data.insert(key.into(), Tag::String(v.into()));
    }

    /// `ScheduleInstruction.supportsConditions`.
    pub(crate) fn supports_conditions(&self) -> bool {
        matches!(
            self.id.as_str(),
            "destination" | "package_delivery" | "package_retrieval"
        )
    }

    /// `IScheduleInput.slotsTargeted`: the editor's item slots.
    pub(crate) fn slots(&self) -> usize {
        match self.id.as_str() {
            "item_threshold" | "fluid_threshold" => 1,
            "redstone_link" => 2,
            _ => 0,
        }
    }

    /// `getItem(slot)`: the item's NBT as saved (`FilterItemStack`, the frequency's stack), None
    /// when empty.
    pub(crate) fn item(&self, slot: usize) -> Option<Compound> {
        let tag = match self.id.as_str() {
            "item_threshold" => self.data.get(&"Item".into()),
            "fluid_threshold" => self.data.get(&"Bucket".into()),
            "redstone_link" => match self.data.get(&"Frequency".into()) {
                Some(Tag::List(l)) => l.get(slot),
                _ => None,
            },
            _ => None,
        };
        match tag {
            Some(Tag::Compound(c)) if !c.is_empty() => Some(c.clone()),
            _ => None,
        }
    }

    /// `setItem(slot, stack)`: an empty stack saves as an empty compound
    /// (`ItemStack.saveOptional`).
    pub(crate) fn set_item(&mut self, slot: usize, item: Option<Compound>) {
        let c = Tag::Compound(item.unwrap_or_default());
        match self.id.as_str() {
            "item_threshold" => {
                self.data.insert("Item".into(), c);
            }
            "fluid_threshold" => {
                self.data.insert("Bucket".into(), c);
            }
            "redstone_link" => {
                let mut pair = match self.data.get(&"Frequency".into()) {
                    Some(Tag::List(l)) if l.len() == 2 => l.clone(),
                    _ => vec![Tag::Compound(Compound::new()); 2],
                };
                if let Some(s) = pair.get_mut(slot) {
                    *s = c;
                }
                self.data.insert("Frequency".into(), Tag::List(pair));
            }
            _ => {}
        }
    }

    /// `TimedWaitCondition.totalWaitTicks`.
    pub(crate) fn wait_ticks(&self) -> i32 {
        let per = [1, 20, 1200][self.int("TimeUnit").rem_euclid(3) as usize];
        self.int("Value").saturating_mul(per)
    }

    /// `write`: `{Id, Data}`.
    fn to_nbt(&self) -> Compound {
        let mut c = Compound::new();
        c.insert(
            "Id".into(),
            Tag::String(format!("create:{}", self.id).as_str().into()),
        );
        c.insert("Data".into(), Tag::Compound(self.data.clone()));
        c
    }

    /// `fromTag`: an instruction of a type not Create's reads as a destination (as Create's
    /// `ScheduleInstruction.fromTag` does), a condition of one is dropped.
    fn from_nbt(c: &Compound, instruction: bool) -> Option<Input> {
        let id = match c.get(&"Id".into()) {
            Some(Tag::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        };
        let path = id.strip_prefix("create:").unwrap_or("");
        let known = if instruction {
            INSTRUCTIONS.contains(&path)
        } else {
            CONDITIONS.contains(&path)
        };
        let data = match c.get(&"Data".into()) {
            Some(Tag::Compound(d)) => d.clone(),
            _ => Compound::new(),
        };
        match (known, instruction) {
            (true, _) => Some(Input {
                id: path.to_owned(),
                data,
            }),
            (false, true) => Some(Input::new("destination")),
            (false, false) => None,
        }
    }
}

/// `ScheduleEntry`: an instruction and, when it supports them, its condition groups.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub instruction: Input,
    pub conditions: Vec<Vec<Input>>,
}

impl Entry {
    fn to_nbt(&self) -> Compound {
        let mut c = Compound::new();
        c.insert(
            "Instruction".into(),
            Tag::Compound(self.instruction.to_nbt()),
        );
        if !self.instruction.supports_conditions() {
            return c;
        }
        let groups = self
            .conditions
            .iter()
            .map(|g| Tag::List(g.iter().map(|i| Tag::Compound(i.to_nbt())).collect()))
            .collect();
        c.insert("Conditions".into(), Tag::List(groups));
        c
    }

    fn from_nbt(c: &Compound) -> Entry {
        let instruction = match c.get(&"Instruction".into()) {
            Some(Tag::Compound(i)) => Input::from_nbt(i, true),
            _ => None,
        }
        .unwrap_or_else(|| Input::new("destination"));
        let mut conditions = Vec::new();
        if instruction.supports_conditions()
            && let Some(Tag::List(groups)) = c.get(&"Conditions".into())
        {
            for g in groups {
                if let Tag::List(list) = g {
                    conditions.push(
                        list.iter()
                            .filter_map(|t| match t {
                                Tag::Compound(t) => Input::from_nbt(t, false),
                                _ => None,
                            })
                            .collect(),
                    );
                }
            }
        }
        Entry {
            instruction,
            conditions,
        }
    }
}

/// `Schedule`: its entries, whether it loops, and the entry it stands at.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Schedule {
    pub entries: Vec<Entry>,
    pub cyclic: bool,
    pub progress: i32,
}

impl Default for Schedule {
    /// `new Schedule()`: no entries, looping.
    fn default() -> Schedule {
        Schedule {
            entries: Vec::new(),
            cyclic: true,
            progress: 0,
        }
    }
}

impl Schedule {
    /// `Schedule.write`.
    pub(crate) fn to_nbt(&self) -> Compound {
        let mut c = Compound::new();
        c.insert(
            "Entries".into(),
            Tag::List(
                self.entries
                    .iter()
                    .map(|e| Tag::Compound(e.to_nbt()))
                    .collect(),
            ),
        );
        c.insert("Cyclic".into(), Tag::Byte(i8::from(self.cyclic)));
        if self.progress > 0 {
            c.insert("Progress".into(), Tag::Int(self.progress));
        }
        c
    }

    /// `Schedule.fromTag`.
    pub(crate) fn from_nbt(c: &Compound) -> Schedule {
        let entries = match c.get(&"Entries".into()) {
            Some(Tag::List(l)) => l
                .iter()
                .filter_map(|t| match t {
                    Tag::Compound(e) => Some(Entry::from_nbt(e)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let cyclic = matches!(c.get(&"Cyclic".into()), Some(Tag::Byte(b)) if *b != 0);
        let progress = match c.get(&"Progress".into()) {
            Some(Tag::Int(p)) => *p,
            _ => 0,
        };
        Schedule {
            entries,
            cyclic,
            progress,
        }
    }

    /// What the engine runs of it (see the module's doc), carrying the schedule's own NBT.
    pub(crate) fn to_engine(&self) -> create_engine::schedule::Schedule {
        use create_engine::schedule::{Condition, Entry as Run};
        let entries = self
            .entries
            .iter()
            .filter(|e| e.instruction.id == "destination")
            .map(|e| Run {
                station: e.instruction.text("Text"),
                conditions: e
                    .conditions
                    .iter()
                    .map(|g| {
                        g.iter()
                            .map(|c| match c.id.as_str() {
                                "delay" => Condition::Delay(c.wait_ticks()),
                                "idle" => Condition::IdleCargo(c.wait_ticks()),
                                _ => Condition::Delay(i32::MAX),
                            })
                            .collect()
                    })
                    .collect(),
            })
            .collect();
        create_engine::schedule::Schedule {
            entries,
            cyclic: self.cyclic,
            source: bytes(&self.to_nbt()),
        }
    }
}

/// A compound as the wire carries it (network NBT).
pub(crate) fn bytes(c: &Compound) -> Vec<u8> {
    Tag::Compound(c.clone())
        .to_network_bytes()
        .unwrap_or_default()
}

/// A compound from its network NBT.
pub(crate) fn compound_of(b: &[u8]) -> Option<Compound> {
    match Tag::from_network_bytes(b, None).ok()?.0 {
        Tag::Compound(c) => Some(c),
        _ => None,
    }
}

/// `ScheduleItem.getSchedule`: None without one.
pub(crate) fn schedule_of(stack: &ItemStack) -> Option<Schedule> {
    match custom(stack).get(&KEY.into()) {
        Some(Tag::Compound(c)) => Some(Schedule::from_nbt(c)),
        _ => None,
    }
}

fn styled(text: &str, colour: &str) -> Text {
    let mut style = mod_api::text::TextStyle::default();
    style.color = Some(colour.to_owned());
    Text::literal(text.to_owned()).with_style(style)
}

/// Whether `stack` is a schedule.
fn is_schedule(host: &dyn Host, stack: &ItemStack) -> bool {
    stack.item().id(host) == ITEM
}

// ---------------------------------------------------------------------------------------------
// The item

/// `ScheduleItem`: used with the main hand, not sneaking, it opens its screen (in the air and
/// on blocks, `useOn` doing what `use` does); its tooltip lists its stops.
pub(super) struct SchedulePart;

#[mod_api::horizon_part]
impl ItemPart for SchedulePart {
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        used(host, event.player, event.hand)
    }

    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        used(host, event.player, event.hand)
    }

    /// `appendHoverText`: each destination's filter, the current one marked when there are
    /// several entries.
    fn tooltip(&self, _host: &dyn mod_api::ClientHost, event: &mut TooltipEvent<'_>) {
        let Some(s) = schedule_of(event.stack) else {
            return;
        };
        for (i, e) in s.entries.iter().enumerate() {
            if e.instruction.id != "destination" {
                continue;
            }
            let current = i as i32 == s.progress && s.entries.len() > 1;
            let (prefix, colour) = if current {
                ("-> ", "yellow")
            } else {
                ("> ", "gold")
            };
            let line = styled(prefix, "gray").append(styled(&e.instruction.text("Text"), colour));
            event.lines.push((line, u32::MAX));
        }
    }
}

/// `ScheduleItem.use`: success (and the screen, on the server) for the main hand when not
/// sneaking; a pass otherwise.
fn used(host: HostSide, player: u128, hand: Hand) -> Option<InteractionResult> {
    if hand != Hand::Main {
        return None;
    }
    match host {
        HostSide::Server(host) => {
            let player = host.player(player)?;
            if sneaking(&*host, player) {
                return None;
            }
            open_screen(host, player);
        }
        HostSide::Client(host) => {
            let player = host.local_player()?;
            if sneaking(&*host, player) {
                return None;
            }
        }
    }
    Some(InteractionResult::Success)
}

/// `player.openMenu(schedule)`: the screen, with the held schedule, a copy of the inventory
/// (for the editor's item slots) and the stations' names nearest first (its suggestions).
fn open_screen(host: &mut dyn ServerHost, player: PlayerRef) {
    let Some(stack) = held(&*host, player, Hand::Main) else {
        return;
    };
    let h: &dyn Host = &*host;
    let inventory = player
        .inventory_slots(h)
        .unwrap_or_default()
        .into_iter()
        .filter(|(slot, _)| *slot < 36)
        .filter_map(|(slot, s)| Some((slot as u8, super::filter_screen::stack_bytes(h, &s)?)))
        .collect();
    let schedule = match custom(&stack).get(&KEY.into()) {
        Some(Tag::Compound(c)) => bytes(c),
        _ => Vec::new(),
    };
    let opened = Opened {
        selected: player.selected_slot(h).unwrap_or(0) as u8,
        inventory,
        schedule,
        stations: stations(host, player),
    };
    net::send_to_player(host, player, schedule_screen::CHANNEL, &opened.encode());
}

/// The names of the stations in the player's level, nearest first, each once
/// (`getViableStations`, without its track graph narrowing).
fn stations(host: &dyn ServerHost, player: PlayerRef) -> Vec<String> {
    let Some(dim) = player
        .level(host)
        .and_then(|l| l.dimension().id(host))
        .and_then(|d| super::super::world::level_index(&d))
    else {
        return Vec::new();
    };
    let at = player.entity().position(host).unwrap_or([0.0; 3]);
    let mut all: Vec<(f64, String)> = super::super::world::with_world(|w| {
        w.dims[dim]
            .railway
            .stations
            .iter()
            .map(|(p, name)| {
                let d: f64 = (0..3)
                    .map(|i| (f64::from(p[i]) + 0.5 - at[i]).powi(2))
                    .sum();
                (d, name.clone())
            })
            .collect()
    })
    .unwrap_or_default();
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut seen = std::collections::HashSet::new();
    all.into_iter()
        .filter(|(_, n)| seen.insert(n.clone()))
        .map(|(_, n)| n)
        .collect()
}

/// `ScheduleEditPacket.handle`: the main hand's schedule takes what the screen wrote, or loses
/// its schedule when it has no entries.
pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some(c) = compound_of(data) else {
        return;
    };
    let Some(mut stack) = held(&*host, player, Hand::Main) else {
        return;
    };
    if !is_schedule(&*host, &stack) {
        return;
    }
    let s = Schedule::from_nbt(&c);
    let value = (!s.entries.is_empty()).then(|| Tag::Compound(s.to_nbt()));
    set_custom(&mut stack, &[(KEY, value)]);
    set_held(host, player, Hand::Main, Some(stack));
}

// ---------------------------------------------------------------------------------------------
// The conductor

/// `ScheduleItemEntityInteraction.interactWithConductor` and `ScheduleItem.handScheduleTo`: a
/// click on someone sitting on a carriage's seat. A schedule is handed to the train when they
/// sit in a conductor's seat; with an empty main hand, a paused schedule resumes or the
/// train's schedule is taken back.
pub(super) fn entity_interact(
    host: HostSide,
    event: &EntityInteractEvent<'_>,
) -> Option<InteractionResult> {
    let host = match host {
        HostSide::Server(host) => host,
        HostSide::Client(host) => return predict(&*host, event),
    };
    let player = host.players().acting_player()?;
    if player.is_spectator(&*host) == Some(true) {
        return None;
    }
    let target = host.entity(u32::try_from(event.entity).ok()?)?;
    let (dim, at) = super::super::trains::seat_of(&*host, target)?;
    let schedule = event
        .stack
        .filter(|s| is_schedule(&*host, s))
        .and_then(schedule_of);
    if let Some(schedule) = schedule {
        hand_to(host, player, event.hand, dim, at, schedule);
        return Some(InteractionResult::Success);
    }
    if event.hand == Hand::Off {
        return None;
    }
    let (has, paused) = super::super::world::with_world(|w| {
        let t = w.dims[dim]
            .railway
            .trains
            .iter()
            .find(|t| t.id == at.train)?;
        Some((
            t.schedule.is_some(),
            t.runtime.paused && !t.runtime.completed,
        ))
    })
    .flatten()?;
    if !has || !super::super::trains::conductor_seat(&*host, dim, at) {
        return None;
    }
    if paused {
        super::super::world::with_world(|w| {
            if let Some(t) = w.dims[dim]
                .railway
                .trains
                .iter_mut()
                .find(|t| t.id == at.train)
            {
                t.runtime.paused = false;
            }
        });
        sound(host, player, "create:confirm", 1.0);
        action_bar(host, player, "create.schedule.continued");
        return Some(InteractionResult::Success);
    }
    if held(&*host, player, event.hand).is_some() {
        sound(host, player, "create:deny", 1.0);
        action_bar(host, player, "create.schedule.remove_with_empty_hand");
        return Some(InteractionResult::Success);
    }
    let taken =
        super::super::world::with_world(|w| w.dims[dim].railway.take_schedule(at.train)).flatten();
    if let Some((s, progress)) = taken {
        // `playItemPickup`.
        let pitch = 1.0 + random(host, player);
        if let (Some(level), Some(pos)) = (player.level(&*host), player.entity().position(&*host)) {
            level.play_sound(
                host,
                "minecraft:entity.item.pickup",
                pos,
                SoundSource::Players,
                0.2,
                pitch,
            );
        }
        action_bar(host, player, "create.schedule.removed_from_train");
        // `returnSchedule`: the schedule as handed over, at the entry the train was at.
        let mut back = compound_of(&s.source)
            .map(|c| Schedule::from_nbt(&c))
            .unwrap_or_default();
        back.progress = progress as i32;
        if let Some(mut stack) = host
            .item_key(&mod_api::Identifier::parse(ITEM).ok()?)
            .map(ItemStack::of)
        {
            set_custom(&mut stack, &[(KEY, Some(Tag::Compound(back.to_nbt())))]);
            give_back(host, player, stack);
        }
    }
    Some(InteractionResult::Success)
}

/// The client's answer, as the server's will be: anyone riding an item display may be on a
/// carriage's seat (see the module's doc).
fn predict(
    host: &dyn mod_api::ClientHost,
    event: &EntityInteractEvent<'_>,
) -> Option<InteractionResult> {
    let target = host.entity(u32::try_from(event.entity).ok()?)?;
    let vehicle = target.vehicle(host)?;
    (vehicle.kind(host)?.as_str() == "minecraft:item_display").then_some(InteractionResult::Success)
}

/// `handScheduleTo` on the server: only from a conductor's seat, to a train without a
/// schedule, of a schedule with stops; then the train follows it and the stack shrinks.
fn hand_to(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    hand: Hand,
    dim: usize,
    at: create_engine::train_controls::Target,
    schedule: Schedule,
) {
    if !super::super::trains::conductor_seat(&*host, dim, at) {
        action_bar(host, player, "create.schedule.non_controlling_seat");
        sound(host, player, "create:deny", 1.0);
        return;
    }
    let has = super::super::world::with_world(|w| {
        w.dims[dim]
            .railway
            .trains
            .iter()
            .find(|t| t.id == at.train)
            .map(|t| t.schedule.is_some())
    })
    .flatten();
    let Some(has) = has else {
        return;
    };
    if has {
        sound(host, player, "create:deny", 1.0);
        action_bar(host, player, "create.schedule.remove_with_empty_hand");
        return;
    }
    if schedule.entries.is_empty() {
        sound(host, player, "create:deny", 1.0);
        action_bar(host, player, "create.schedule.no_stops");
        return;
    }
    let progress = schedule.progress.max(0) as usize;
    let run = schedule.to_engine();
    super::super::world::with_world(|w| w.dims[dim].railway.set_schedule(at.train, run, progress));
    sound(host, player, "create:confirm", 1.0);
    let mut style = mod_api::text::TextStyle::default();
    style.color = Some("green".to_owned());
    let line = Text::translate("create.schedule.applied_to_train", Vec::new()).with_style(style);
    let _ = player.action_bar(host, line);
    // `pStack.shrink(1)`, in creative too.
    super::shrink_held(host, player, hand, 1);
}

/// `AllSoundEvents.X.playOnServer(level, player.blockPosition(), 1, pitch)`.
fn sound(host: &mut dyn ServerHost, player: PlayerRef, name: &str, pitch: f32) {
    let (Some(level), Some(p)) = (player.level(&*host), player.entity().position(&*host)) else {
        return;
    };
    let at = p.map(|v| v.floor() + 0.5);
    level.play_sound(host, name, at, SoundSource::Players, 1.0, pitch);
}

/// `level.random.nextFloat()`.
fn random(host: &mut dyn ServerHost, player: PlayerRef) -> f32 {
    use mod_api::RandomSource;
    player
        .level(&*host)
        .and_then(|l| l.with_random(host, |r| r.next_float()))
        .unwrap_or(0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schedule using every type, with its items.
    fn every_type() -> Schedule {
        let mut item = Input::new("item_threshold");
        let mut stack = Compound::new();
        stack.insert("id".into(), Tag::String("minecraft:iron_ingot".into()));
        stack.insert("count".into(), Tag::Int(1));
        item.set_item(0, Some(stack.clone()));
        let mut link = Input::new("redstone_link");
        link.set_item(1, Some(stack));
        link.set_int("Inverted", 1);
        let mut dest = Input::new("destination");
        dest.set_text("Text", "Mine, Platform *");
        Schedule {
            entries: vec![
                Entry {
                    instruction: dest,
                    conditions: vec![
                        CONDITIONS[..5].iter().map(|c| Input::new(c)).collect(),
                        vec![item, link],
                        CONDITIONS[5..].iter().map(|c| Input::new(c)).collect(),
                    ],
                },
                Entry {
                    instruction: Input::new("throttle"),
                    conditions: Vec::new(),
                },
                Entry {
                    instruction: Input::new("package_delivery"),
                    conditions: vec![vec![Input::new("delay")]],
                },
            ],
            cyclic: false,
            progress: 1,
        }
    }

    #[test]
    fn a_schedule_reads_back_as_written_in_creates_layout() {
        let s = every_type();
        let nbt = s.to_nbt();
        // Create's keys: `Entries`, `Cyclic`, `Progress`; `Instruction`/`Conditions`; `Id`/`Data`.
        let Some(Tag::List(entries)) = nbt.get(&"Entries".into()) else {
            panic!("no entries");
        };
        let Tag::Compound(first) = &entries[0] else {
            panic!()
        };
        let Some(Tag::Compound(i)) = first.get(&"Instruction".into()) else {
            panic!()
        };
        assert_eq!(
            i.get(&"Id".into()),
            Some(&Tag::String("create:destination".into()))
        );
        // A throttle supports no conditions: none are written.
        let Tag::Compound(second) = &entries[1] else {
            panic!()
        };
        assert!(second.get(&"Conditions".into()).is_none());
        assert_eq!(nbt.get(&"Progress".into()), Some(&Tag::Int(1)));
        assert_eq!(Schedule::from_nbt(&nbt), s);
        // Through the wire too.
        assert_eq!(Schedule::from_nbt(&compound_of(&bytes(&nbt)).unwrap()), s);
    }

    #[test]
    fn unknown_types_read_as_create_reads_them() {
        let mut bad = Compound::new();
        bad.insert("Id".into(), Tag::String("othermod:teleport".into()));
        let mut entry = Compound::new();
        entry.insert("Instruction".into(), Tag::Compound(bad.clone()));
        entry.insert(
            "Conditions".into(),
            Tag::List(vec![Tag::List(vec![
                Tag::Compound(bad),
                Tag::Compound(Input::new("idle").to_nbt()),
            ])]),
        );
        let e = Entry::from_nbt(&entry);
        assert_eq!(e.instruction, Input::new("destination"));
        assert_eq!(e.conditions, vec![vec![Input::new("idle")]]);
        // A missing `Cyclic` is false, as `getBoolean` reads it; a new schedule loops.
        assert!(!Schedule::from_nbt(&Compound::new()).cyclic);
        assert!(Schedule::default().cyclic);
    }

    #[test]
    fn the_train_runs_destinations_with_their_waits() {
        use create_engine::schedule::Condition;
        let mut s = every_type();
        let mut minutes = Input::new("delay");
        minutes.set_int("Value", 2);
        minutes.set_int("TimeUnit", 2);
        s.entries[0].conditions = vec![
            vec![minutes, Input::new("idle")],
            vec![Input::new("powered")],
        ];
        let run = s.to_engine();
        // The throttle and the package delivery are left out.
        assert_eq!(run.entries.len(), 1);
        assert_eq!(run.entries[0].station, "Mine, Platform *");
        assert_eq!(
            run.entries[0].conditions,
            vec![
                vec![Condition::Delay(2400), Condition::IdleCargo(100)],
                vec![Condition::Delay(i32::MAX)],
            ]
        );
        assert!(!run.cyclic);
        assert_eq!(
            compound_of(&run.source).map(|c| Schedule::from_nbt(&c)),
            Some(s)
        );
    }

    #[test]
    fn items_save_as_create_saves_an_empty_stack() {
        let mut link = Input::new("redstone_link");
        assert_eq!(link.item(0), None);
        link.set_item(0, None);
        assert_eq!(
            link.data.get(&"Frequency".into()),
            Some(&Tag::List(vec![Tag::Compound(Compound::new()); 2]))
        );
        assert_eq!(link.item(1), None);
    }
}
