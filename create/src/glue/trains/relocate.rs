//! Moving a train with the wrench (`TrainRelocator`): a wrench's click on a standing carriage
//! picks its train, the next click on a track within 24 blocks of where it was picked puts the
//! train there, facing the way the player looks; sneaking or letting go of the wrench gives
//! up. Create picks and previews on the client (the green and red line of where the wheels
//! would go); here the server keeps the pick, and the action bar says what a click does.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::railway::{railway_ready, send_trains};
use mod_api::{PlayerRef, ServerHost, Text};
use std::cell::RefCell;
use std::collections::HashMap;

/// A player's pick: the level, the train, and where the carriage was clicked.
#[derive(Clone, Copy)]
struct Pick {
    dim: usize,
    train: u32,
    origin: [f64; 3],
}

thread_local! {
    static PICKS: RefCell<HashMap<u128, Pick>> = RefCell::new(HashMap::new());
}

pub fn forget() {
    PICKS.with_borrow_mut(HashMap::clear);
}

fn say(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    key: &str,
    args: Vec<Text>,
    colour: Option<&str>,
) {
    let mut text = Text::translate(key, args);
    if let Some(c) = colour {
        let mut style = mod_api::text::TextStyle::default();
        style.color = Some(c.to_owned());
        text = text.with_style(style);
    }
    super::action_bar(host, player, text);
}

/// `carriageWrenched`: the wrench's click on train `train` at `origin`. A moving train can't be
/// picked (`train.cannot_relocate_moving`).
pub fn wrenched(
    host: &mut dyn ServerHost,
    dim: usize,
    player: PlayerRef,
    train: u32,
    origin: [f64; 3],
) {
    let found = with_world(|w| {
        w.dims[dim]
            .railway
            .trains
            .iter()
            .find(|t| t.id == train)
            .map(|t| (t.train.speed != 0.0, t.name.clone()))
    })
    .flatten();
    let Some((moving, name)) = found else {
        return;
    };
    if moving {
        say(
            host,
            player,
            "create.train.cannot_relocate_moving",
            Vec::new(),
            Some("red"),
        );
        return;
    }
    PICKS.with_borrow_mut(|p| p.insert(player.uuid(), Pick { dim, train, origin }));
    let name = if name.is_empty() {
        Text::translate("create.train.unnamed", Vec::new())
    } else {
        Text::literal(name)
    };
    say(host, player, "create.train.relocate", vec![name], None);
}

/// Whether `player` has a train picked.
pub fn picking(player: PlayerRef) -> bool {
    PICKS.with_borrow(|p| p.contains_key(&player.uuid()))
}

/// The wrench's click on track `pos` with a train picked (`onClicked`, `TrainRelocationPacket`):
/// sneaking gives up; too far from the pick is refused; else the train goes there if it fits.
/// True when the click was the relocator's.
pub fn track_clicked(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) -> bool {
    let Some(pick) = PICKS.with_borrow(|p| p.get(&player.uuid()).copied()) else {
        return false;
    };
    let host = &mut *lv.host;
    let wrench = crate::glue::items::held(&*host, player, mod_api::Hand::Main)
        .is_some_and(|s| s.item().id(&*host) == "create:wrench");
    if crate::glue::items::sneaking(&*host, player) || !wrench || pick.dim != lv.dim {
        PICKS.with_borrow_mut(|p| p.remove(&player.uuid()));
        say(
            host,
            player,
            "create.train.relocate.abort",
            Vec::new(),
            Some("red"),
        );
        return true;
    }
    // `canInteractWithBlock(relocatingOrigin, 24)`.
    let near = player.entity().position(&*host).is_some_and(|p| {
        let d: f64 = (0..3).map(|i| (p[i] - pick.origin[i]).powi(2)).sum();
        d < 24.0 * 24.0
    });
    if !near {
        say(
            host,
            player,
            "create.train.relocate.too_far",
            Vec::new(),
            Some("red"),
        );
        return true;
    }
    // The way the player looks at the clicked track (`lookAngle`): from their eye to it.
    let look = player
        .entity()
        .position(&*host)
        .map_or([0.0, 0.0, 1.0], |p| {
            [
                f64::from(pos[0]) + 0.5 - p[0],
                0.0,
                f64::from(pos[2]) + 0.5 - p[2],
            ]
        });
    let dim = lv.dim;
    railway_ready(dim);
    let moved = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let net = &r.net;
        let t = r.trains.iter_mut().find(|t| t.id == pick.train)?;
        let was = t.column;
        if t.train.speed != 0.0 || !t.train.relocate(net, pos, look) {
            return Some(None);
        }
        t.runtime.interrupted();
        t.column = t.column_on(net);
        t.fresh = true;
        Some(Some((was, t.column)))
    })
    .flatten();
    match moved {
        Some(Some((was, now))) => {
            PICKS.with_borrow_mut(|p| p.remove(&player.uuid()));
            super::super::persist::mark_unsaved(dim, [was.0, was.1]);
            super::super::persist::mark_unsaved(dim, [now.0, now.1]);
            say(
                lv.host,
                player,
                "create.train.relocate.success",
                Vec::new(),
                Some("green"),
            );
            send_trains(lv, &[pick.train], true);
        }
        Some(None) => {
            say(
                lv.host,
                player,
                "create.train.relocate.invalid",
                Vec::new(),
                Some("red"),
            );
        }
        None => {
            PICKS.with_borrow_mut(|p| p.remove(&player.uuid()));
        }
    }
    true
}
