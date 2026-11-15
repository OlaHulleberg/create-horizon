//! Create's `GlobalTrainDisplayData`: every 100 ticks, the arrival predictions of each running
//! train's schedule (`ScheduleRuntime.submitPredictions`), by destination, for the station
//! summary display source; and the stations' display links told when the train standing there
//! asks for it (`StationBlockEntity`'s `displayLinkUpdateRequested`).

use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::display::{Arrival, Config};
use create_engine::schedule::Whereabouts;
use create_engine::trains::{TOP_SPEED, TURN_TOP_SPEED};
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default)]
struct Arrivals {
    /// The game time it was last refreshed at.
    refreshed: Option<i64>,
    by_destination: HashMap<String, Vec<Arrival>>,
}

thread_local! {
    static ARRIVALS: RefCell<Arrivals> = RefCell::new(Arrivals::default());
}

/// The world closed: nothing is known.
pub(super) fn forget() {
    ARRIVALS.with_borrow_mut(|a| *a = Arrivals::default());
}

/// `GlobalRailwayManager.tick`: `refresh` when the game time is a multiple of 100.
pub(super) fn tick(lv: &ServerLevel<'_>) {
    let now = lv.game_time();
    if now % 100 != 0 || ARRIVALS.with_borrow(|a| a.refreshed == Some(now)) {
        return;
    }
    let by_destination = with_world(|w| {
        let mut map: HashMap<String, Vec<Arrival>> = HashMap::new();
        for d in &w.dims {
            let r = &d.railway;
            for t in &r.trains {
                // `train.runtime.paused || getSchedule() == null`, `derailed || graph == null`.
                let Some(schedule) = &t.schedule else {
                    continue;
                };
                if t.runtime.paused || !t.train.bound {
                    continue;
                }
                let at = Whereabouts {
                    station: t.train.at_station.and_then(|p| r.stations.get(&p).cloned()),
                    destination: t.train.nav.as_ref().and_then(|n| {
                        Some((
                            r.stations.get(&n.station)?.clone(),
                            n.distance,
                            n.started_at,
                        ))
                    }),
                    speed: (t.train.throttle * TOP_SPEED).min((TOP_SPEED + TURN_TOP_SPEED) / 2.0),
                };
                let name = if t.name.is_empty() {
                    "Unnamed Train".to_owned()
                } else {
                    t.name.clone()
                };
                for p in t.runtime.predict(schedule, &at) {
                    map.entry(p.destination.clone()).or_default().push(Arrival {
                        ticks: p.ticks,
                        train: name.clone(),
                        title: p.title,
                        destination: p.destination,
                    });
                }
            }
        }
        map
    })
    .unwrap_or_default();
    ARRIVALS.with_borrow_mut(|a| {
        a.refreshed = Some(now);
        a.by_destination = by_destination;
    });
}

/// `GlobalTrainDisplayData.prepare(filter, ..)`, every match (the source keeps the board's rows).
pub(super) fn arrivals(filter: &str) -> Vec<Arrival> {
    ARRIVALS.with_borrow(|a| {
        create_engine::display::prepare_arrivals(&a.by_destination, filter, usize::MAX)
    })
}

/// `StationSummaryDisplaySource.populateData`: the columns' defaults, and the station's name as
/// the filter when it has none.
pub(super) fn populate(config: &mut Config, station: Option<&str>) {
    config.platform_column.get_or_insert(3);
    config.name_column.get_or_insert(50);
    if config.filter.is_none()
        && let Some(name) = station
    {
        config.filter = Some(name.to_owned());
    }
}

/// The name of the station at `pos`, if it is one.
pub(super) fn station_name(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<String> {
    let dim = lv.dim;
    with_world(|w| w.dims[dim].railway.stations.get(&pos).cloned()).flatten()
}

/// The stations whose standing train asked for its display links to refresh
/// (`displayLinkUpdateRequested`, cleared as they are told).
pub(super) fn stations_to_tell(dim: usize) -> Vec<[i32; 3]> {
    with_world(|w| {
        let mut out = Vec::new();
        for t in &mut w.dims[dim].railway.trains {
            if let Some(at) = t.train.at_station
                && t.runtime.display_update
                && t.schedule.is_some()
            {
                t.runtime.display_update = false;
                out.push(at);
            }
        }
        out
    })
    .unwrap_or_default()
}
