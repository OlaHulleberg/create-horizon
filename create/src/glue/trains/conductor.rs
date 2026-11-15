//! Who runs a train's schedule: a conductor, a mob sitting on a carriage seat behind train
//! controls (`CarriageContraption.conductorSeats`), handed a schedule by a click
//! (`ScheduleItem.handScheduleTo`, `ScheduleItemEntityInteraction`); or a station holding a
//! schedule (`StationBlockEntity.applyAutoSchedule`): one dropped on it, given to each train
//! that arrives there, taken back by a click.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::schedule_screen::{schedule_of, with_schedule};
use crate::glue::items;
use create_engine::schedule::{Runtime, Schedule};
use mod_api::{
    EntityInteractEvent, EntityRef, Hand, InteractionResult, PlayerRef, ServerHost, SoundSource,
    Text,
};

/// A sound at the player (`AllSoundEvents.CONFIRM` / `DENY`, `playOnServer`).
fn sound_at(host: &mut dyn ServerHost, player: PlayerRef, id: &str, volume: f32, pitch: f32) {
    let Some(level) = player.level(&*host) else {
        return;
    };
    if let Some(at) = player.entity().position(&*host) {
        level.play_sound(host, id, at, SoundSource::Players, volume, pitch);
    }
}

fn say(host: &mut dyn ServerHost, player: PlayerRef, key: &str) {
    super::action_bar(host, player, Text::translate(key, Vec::new()));
}

/// `ScheduleRuntime.setSchedule`: the train runs `schedule` from its saved progress; a station's
/// is an auto-schedule.
pub fn set_schedule(dim: usize, train: u32, schedule: Schedule, auto: bool) -> bool {
    with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let t = r.trains.iter_mut().find(|t| t.id == train)?;
        t.runtime = Runtime::for_schedule(&schedule, auto);
        t.train.nav = None;
        t.schedule = Some(schedule);
        Some(t.column)
    })
    .flatten()
    .map(|c| super::super::persist::mark_unsaved(dim, [c.0, c.1]))
    .is_some()
}

/// `ScheduleRuntime.returnSchedule`: the train's schedule taken off it (`discardSchedule`),
/// with its progress; None for an auto-schedule, which is not given back.
pub fn take_schedule(dim: usize, train: u32) -> Option<Option<Schedule>> {
    with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let t = r.trains.iter_mut().find(|t| t.id == train)?;
        let mut s = t.schedule.take()?;
        s.saved_progress = t.runtime.current;
        let auto = t.runtime.auto;
        t.runtime = Runtime::new();
        t.runtime.paused = true;
        t.train.nav = None;
        super::super::persist::mark_unsaved(dim, [t.column.0, t.column.1]);
        Some((!auto).then_some(s))
    })
    .flatten()
}

/// `CarriageContraption.inControl` for the carriage seat at `local` of carriage `car`: train
/// controls next to it along the train, facing it.
fn conductor_seat(
    dim: usize,
    train: u32,
    car: usize,
    local: [i32; 3],
    host: &dyn ServerHost,
) -> bool {
    let found = with_world(|w| {
        let t = w.dims[dim].railway.trains.iter().find(|t| t.id == train)?;
        let blocks = t.cars.get(car)?.clone();
        Some((blocks, t.heading))
    })
    .flatten();
    let Some((blocks, heading)) = found else {
        return false;
    };
    let controls = super::super::states::key(create_engine::Block::Controls);
    let axis = if heading[0] != 0 { 0 } else { 2 };
    [1, -1].into_iter().any(|s| {
        let mut d = [0, 0, 0];
        d[axis] = s;
        let at = [local[0] + d[0], local[1], local[2] + d[2]];
        // The way back toward the seat (`direction.getOpposite()`), by its name.
        let back = match (axis, -s) {
            (0, 1) => "east",
            (0, _) => "west",
            (_, 1) => "south",
            _ => "north",
        };
        blocks.iter().any(|(o, id)| {
            *o == at
                && super::car_state(*id).is_some_and(|st| {
                    st.block() == controls
                        && super::property(host, st, "facing").as_deref() == Some(back)
                })
        })
    })
}

/// `ScheduleItemEntityInteraction`: a click on a mob sitting on a carriage seat. With a
/// schedule, it is handed to the train if the mob is its conductor (`handScheduleTo`); with an
/// empty hand, a train paused by its driver runs on, else its schedule comes back.
pub fn interact(
    host: &mut dyn ServerHost,
    event: &mut EntityInteractEvent<'_>,
) -> Option<InteractionResult> {
    let player = host.players().acting_player()?;
    let target: EntityRef = host.entity(event.entity as u32)?;
    let seat = super::riding::seat_of(&*host, target)?;
    let dimension = player.level(&*host)?.dimension().id(&*host)?;
    let dim = ServerLevel::new(host, &dimension)?.dim;
    let holding = event
        .stack
        .filter(|s| s.item().id(&*host) == "create:schedule")
        .cloned();
    let conductor = conductor_seat(dim, seat.train, usize::from(seat.car), seat.local, &*host);
    let has_schedule = with_world(|w| {
        w.dims[dim]
            .railway
            .trains
            .iter()
            .find(|t| t.id == seat.train)
            .map(|t| (t.schedule.is_some(), t.runtime.paused, t.runtime.completed))
    })
    .flatten()?;
    if let Some(stack) = holding {
        let schedule = schedule_of(&stack)?;
        if !conductor {
            say(host, player, "create.schedule.non_controlling_seat");
            sound_at(host, player, "create:deny", 1.0, 1.0);
            return Some(InteractionResult::SuccessServer);
        }
        if has_schedule.0 {
            sound_at(host, player, "create:deny", 1.0, 1.0);
            say(host, player, "create.schedule.remove_with_empty_hand");
            return Some(InteractionResult::SuccessServer);
        }
        if schedule.entries.is_empty() {
            sound_at(host, player, "create:deny", 1.0, 1.0);
            say(host, player, "create.schedule.no_stops");
            return Some(InteractionResult::SuccessServer);
        }
        set_schedule(dim, seat.train, schedule, false);
        crate::glue::advancements::award(host, player, "conductor");
        sound_at(host, player, "create:confirm", 1.0, 1.0);
        let mut style = mod_api::text::TextStyle::default();
        style.color = Some("green".to_owned());
        super::action_bar(
            host,
            player,
            Text::translate("create.schedule.applied_to_train", Vec::new()).with_style(style),
        );
        items::shrink_held(host, player, event.hand, 1);
        return Some(InteractionResult::SuccessServer);
    }
    if event.hand == Hand::Off || !has_schedule.0 || !conductor {
        return None;
    }
    let (_, paused, completed) = has_schedule;
    if paused && !completed {
        with_world(|w| {
            if let Some(t) = w.dims[dim]
                .railway
                .trains
                .iter_mut()
                .find(|t| t.id == seat.train)
            {
                t.runtime.paused = false;
            }
        });
        sound_at(host, player, "create:confirm", 1.0, 1.0);
        say(host, player, "create.schedule.continued");
        return Some(InteractionResult::SuccessServer);
    }
    if event.stack.is_some_and(|s| s.count() > 0) {
        sound_at(host, player, "create:deny", 1.0, 1.0);
        say(host, player, "create.schedule.remove_with_empty_hand");
        return Some(InteractionResult::SuccessServer);
    }
    let auto = with_world(|w| {
        w.dims[dim]
            .railway
            .trains
            .iter()
            .find(|t| t.id == seat.train)
            .is_some_and(|t| t.runtime.auto)
    })
    .unwrap_or(false);
    give_back_schedule(host, player, dim, seat.train);
    say(
        host,
        player,
        if auto {
            "create.schedule.auto_removed_from_train"
        } else {
            "create.schedule.removed_from_train"
        },
    );
    Some(InteractionResult::SuccessServer)
}

/// A schedule item holding `s` into `player`'s inventory (`placeItemBackInInventory`), with
/// `playItemPickup`'s sound.
pub fn give_schedule(host: &mut dyn ServerHost, player: PlayerRef, s: &Schedule) {
    if let Some(key) = mod_api::Identifier::parse("create:schedule")
        .ok()
        .and_then(|i| host.item_key(&i))
    {
        let stack = with_schedule(&mod_api::ItemStack::of(key), s);
        items::give_back(host, player, stack);
    }
    sound_at(host, player, "minecraft:entity.item.pickup", 0.2, 1.5);
}

static HAPPY_VILLAGER: std::sync::OnceLock<mod_api::ParticleKey> = std::sync::OnceLock::new();
static END_ROD: std::sync::OnceLock<mod_api::ParticleKey> = std::sync::OnceLock::new();

pub fn register(r: &mut dyn mod_api::Registrar) {
    let _ = HAPPY_VILLAGER.set(r.particle_key("minecraft:happy_villager"));
    let _ = END_ROD.set(r.particle_key("minecraft:end_rod"));
}

thread_local! {
    /// The stations a train stood at as of the last tick, by level (`trainPresent`).
    static PRESENT: std::cell::RefCell<std::collections::HashMap<usize, std::collections::HashSet<[i32; 3]>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

pub fn forget() {
    PRESENT.with_borrow_mut(std::collections::HashMap::clear);
}

/// `applyAutoSchedule`: the train standing at station `pos` runs the schedule it holds. With
/// `now`, whatever stood there; else only when it newly arrived (the level's tick).
pub fn apply_auto(dim: usize, pos: [i32; 3], now: bool) -> bool {
    let _ = now;
    let applied = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let s = r.auto_schedules.get(&pos)?.clone();
        if s.entries.is_empty() {
            return None;
        }
        let t = r
            .trains
            .iter_mut()
            .find(|t| t.train.bound && t.train.at_station == Some(pos))?;
        t.runtime = Runtime::for_schedule(&s, true);
        t.train.nav = None;
        t.schedule = Some(s);
        Some(t.column)
    })
    .flatten();
    if let Some(c) = applied {
        super::super::persist::mark_unsaved(dim, [c.0, c.1]);
        crate::glue::advancements::award_later(dim, pos, "conductor");
    }
    applied.is_some()
}

/// Each tick: a train newly standing at a station holding a schedule takes it (`newlyArrived`
/// in `StationBlockEntity.tick`), with its sound and particles.
pub fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now: std::collections::HashSet<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .railway
            .trains
            .iter()
            .filter(|t| t.train.bound)
            .filter_map(|t| t.train.at_station)
            .collect()
    })
    .unwrap_or_default();
    let newly: Vec<[i32; 3]> = PRESENT.with_borrow_mut(|p| {
        let before = p.entry(dim).or_default();
        let newly = now.difference(before).copied().collect();
        *before = now;
        newly
    });
    for pos in newly {
        if !apply_auto(dim, pos, false) {
            continue;
        }
        let at = [
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 1.0,
            f64::from(pos[2]) + 0.5,
        ];
        lv.level.play_sound(
            &mut *lv.host,
            "create:confirm",
            at,
            SoundSource::Blocks,
            1.0,
            1.0,
        );
        for (key, at, spread, speed, count) in [
            (&HAPPY_VILLAGER, at, [0.35, 0.05, 0.35], 1.0, 8),
            (
                &END_ROD,
                [at[0], at[1] + 0.25, at[2]],
                [0.05, 1.0, 0.05],
                0.005,
                10,
            ),
        ] {
            if let Some(&k) = key.get() {
                let options = mod_api::ParticleOptions::simple(k);
                let _ = lv.level.send_particles(
                    &mut *lv.host,
                    &options,
                    None,
                    at,
                    spread,
                    speed,
                    count,
                );
            }
        }
    }
    // The schedules of stations taken away drop where they stood.
    let dropped = with_world(|w| std::mem::take(&mut w.dims[dim].railway.dropped_schedules))
        .unwrap_or_default();
    for (pos, s) in dropped {
        drop_schedule(lv, pos, &s);
    }
}

/// A schedule item holding `s` dropped at block `pos` (`Containers.dropItemStack`).
pub fn drop_schedule(lv: &mut ServerLevel<'_>, pos: [i32; 3], s: &Schedule) {
    let Some(key) = mod_api::Identifier::parse("create:schedule")
        .ok()
        .and_then(|i| lv.host.item_key(&i))
    else {
        return;
    };
    let stack = with_schedule(&mod_api::ItemStack::of(key), s);
    let p = pos.map(|v| f64::from(v) + 0.5);
    let _ = lv
        .level
        .spawn_item(&mut *lv.host, p[0], p[1], p[2], stack, 10);
}

/// The train's schedule into `player`'s inventory (`placeItemBackInInventory`), with
/// `playItemPickup`'s sound; an auto-schedule is just dropped.
pub fn give_back_schedule(host: &mut dyn ServerHost, player: PlayerRef, dim: usize, train: u32) {
    let Some(taken) = take_schedule(dim, train) else {
        return;
    };
    match taken {
        Some(s) => give_schedule(host, player, &s),
        None => sound_at(host, player, "minecraft:entity.item.pickup", 0.2, 1.5),
    }
}
