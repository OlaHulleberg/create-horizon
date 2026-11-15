//! Postboxes joined to train stations (`GlobalStation.connectedPorts`, `GlobalPackagePort`,
//! `TrainStationFrogportTarget`). A postbox in hand used on a station chooses it
//! (`PackagePortTargetSelectionHandler.onUse`); the next postbox placed within reach
//! (`packagePortRange`, no "can't reach down" for a postbox) joins it with its address.
//!
//! While a train stands at a station (on arrival and every 21 ticks, `runMailTransfer`), each
//! carriage takes the packages of the station's postboxes that are not for that postbox's
//! address, and gives each package it holds to the first postbox whose address takes it. A
//! postbox not loaded is reached through its offline buffer (`offlineBuffer`, `primed`), which
//! fills it again when it loads (`restoreOfflineBuffer`) and follows what it holds while loaded
//! (`saveOfflineBuffer`). The joins are kept in the overworld's attachment
//! `create:station_ports`.
//!
//! The schedule's package instructions find their stations here (`package_stations`):
//! `DeliverPackagesInstruction` the stations with a postbox for a package aboard,
//! `FetchPackagesInstruction` the stations whose postboxes hold packages to send matching its
//! filter.

use super::{body, parcel, with};
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::package;
use mod_api::{AttachmentCodec, AttachmentType, Compound, Holder, ItemStack, PlayerRef, Tag};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

const SLOTS: usize = create_engine::port::SLOTS;

/// A postbox joined to a station: where, the address it took, its offline buffer and whether
/// that has news for it.
#[derive(Clone, Debug)]
pub(super) struct StationPort {
    pub pos: [i32; 3],
    pub address: String,
    pub buffer: Vec<Option<ItemStack>>,
    pub primed: bool,
}

/// A station by level and position.
type At = (usize, [i32; 3]);

/// A train's station last tick and the ticks since its last transfer.
type Standing = ([i32; 3], u32);

thread_local! {
    static PORTS: RefCell<HashMap<At, Vec<StationPort>>> = RefCell::new(HashMap::new());
    /// Each player's chosen station (`activePackageTarget`), by level.
    static CHOSEN: RefCell<HashMap<u128, At>> = RefCell::new(HashMap::new());
    /// Each train's station last tick and the ticks since its last transfer.
    static AT: RefCell<HashMap<(usize, u32), Standing>> = RefCell::new(HashMap::new());
    static LOADED: Cell<bool> = const { Cell::new(false) };
    static DIRTY: Cell<bool> = const { Cell::new(false) };
}

static SAVED: AttachmentType<Compound> = AttachmentType {
    namespace: "create",
    name: "station_ports",
    default: Compound::new,
    codec: Some(AttachmentCodec {
        encode: |data| Ok(data.clone()),
        decode: |data| Ok(data.clone()),
    }),
    copy_on_death: false,
    sync: false,
};

pub(super) fn reset() {
    PORTS.with_borrow_mut(HashMap::clear);
    CHOSEN.with_borrow_mut(HashMap::clear);
    AT.with_borrow_mut(HashMap::clear);
    LOADED.set(false);
    DIRTY.set(false);
}

/// `onUse` on the server: `player` chose the station at `station`.
pub(super) fn choose(player: PlayerRef, dim: usize, station: [i32; 3]) {
    CHOSEN.with_borrow_mut(|c| c.insert(player.uuid(), (dim, station)));
}

/// `flushSettings` for a postbox placed at `pos` by `player`: joined to the station chosen when
/// it is in reach (`validateDiff` for a postbox: within `packagePortRange` of the placed block's
/// bottom centre), else why not. False when no station was chosen.
pub(super) fn placed(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) -> bool {
    let Some(station) = CHOSEN.with_borrow_mut(|c| c.remove(&player.uuid())) else {
        return false;
    };
    if station.0 != lv.dim {
        return false;
    }
    let s = station.1;
    let source = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]),
        f64::from(pos[2]) + 0.5,
    ];
    let target = s.map(|v| f64::from(v) + 0.5);
    let d: f64 = (0..3).map(|i| (target[i] - source[i]).powi(2)).sum();
    if d.sqrt() > 5.0 {
        let mut style = mod_api::text::TextStyle::default();
        style.color = Some("#ff7171".to_owned());
        let _ = player.action_bar(
            lv.host,
            mod_api::Text::translate("create.package_port.too_far", Vec::new()).with_style(style),
        );
        return true;
    }
    let dim = lv.dim;
    with(dim, |pk| {
        if let Some(p) = pk.ports.get_mut(&pos) {
            p.station = Some(s);
        }
    });
    super::mark_unsaved(dim, &[pos]);
    attach(dim, pos);
    true
}

/// `StationBlockEntity.attachPackagePort` for the postbox at `pos` (loaded, with a station): a
/// new join takes its address; an old one fills it from its offline buffer if that has news.
pub(super) fn attach(dim: usize, pos: [i32; 3]) {
    let Some((station, address)) = with(dim, |pk| {
        let p = pk.ports.get(&pos)?;
        Some((p.station?, p.filter.clone()))
    }) else {
        return;
    };
    let restore = PORTS.with_borrow_mut(|m| {
        let list = m.entry((dim, station)).or_default();
        match list.iter_mut().find(|p| p.pos == pos) {
            None => {
                list.push(StationPort {
                    pos,
                    address,
                    buffer: vec![None; SLOTS],
                    primed: false,
                });
                None
            }
            Some(p) if p.primed => {
                p.primed = false;
                Some(p.buffer.clone())
            }
            Some(_) => None,
        }
    });
    DIRTY.set(true);
    if let Some(buffer) = restore {
        with(dim, |pk| {
            if let Some(port) = pk.ports.get_mut(&pos) {
                for (slot, s) in port.items.iter_mut().zip(buffer) {
                    *slot = s.map(parcel);
                }
                pk.touch(pos);
                pk.wake(pos);
            }
        });
        super::mark_unsaved(dim, &[pos]);
    }
}

/// `filterChanged`: the postbox at `pos` (joined to a station) joins it again with its address
/// now (`deregister`, `register`: a new join, its offline buffer empty).
pub(super) fn filter_changed(dim: usize, pos: [i32; 3]) {
    let Some((station, filter)) = with(dim, |pk| {
        pk.ports
            .get(&pos)
            .and_then(|p| Some((p.station?, p.filter.clone())))
    }) else {
        return;
    };
    let same = PORTS.with_borrow(|m| {
        m.get(&(dim, station))
            .and_then(|l| l.iter().find(|p| p.pos == pos))
            .is_some_and(|p| p.address == filter)
    });
    if same {
        return;
    }
    detach(dim, pos);
    attach(dim, pos);
}

/// `removePackagePort`: the postbox at `pos` went.
pub(super) fn detach(dim: usize, pos: [i32; 3]) {
    PORTS.with_borrow_mut(|m| {
        for (k, list) in m.iter_mut() {
            if k.0 == dim {
                list.retain(|p| p.pos != pos);
            }
        }
        m.retain(|_, l| !l.is_empty());
    });
    DIRTY.set(true);
}

/// `saveOfflineBuffer`: the postbox at `pos` holds `items` now.
pub(super) fn save_buffer(dim: usize, pos: [i32; 3], items: &[Option<ItemStack>]) {
    PORTS.with_borrow_mut(|m| {
        for (k, list) in m.iter_mut() {
            if k.0 != dim {
                continue;
            }
            for p in list.iter_mut().filter(|p| p.pos == pos) {
                if p.buffer != items {
                    p.buffer = items.to_vec();
                    DIRTY.set(true);
                }
            }
        }
    });
}

/// A postbox's slots: its own while it is loaded, else its offline buffer.
fn slots_of(dim: usize, port: &StationPort) -> (Vec<Option<ItemStack>>, bool) {
    let loaded = with(dim, |pk| {
        pk.ports.get(&port.pos).map(|p| {
            p.items
                .iter()
                .map(|s| s.as_ref().map(|b| b.stack.clone()))
                .collect()
        })
    });
    match loaded {
        Some(items) => (items, true),
        None => (port.buffer.clone(), false),
    }
}

/// Writes a postbox's slots back: into it while loaded, else into its buffer (primed).
fn put_slots(
    dim: usize,
    station: [i32; 3],
    pos: [i32; 3],
    items: Vec<Option<ItemStack>>,
    loaded: bool,
) {
    if loaded {
        with(dim, |pk| {
            if let Some(port) = pk.ports.get_mut(&pos) {
                for (slot, s) in port.items.iter_mut().zip(items.iter()) {
                    *slot = s.clone().map(parcel);
                }
                pk.touch(pos);
                pk.wake(pos);
            }
        });
        super::mark_unsaved(dim, &[pos]);
    }
    PORTS.with_borrow_mut(|m| {
        if let Some(p) = m
            .get_mut(&(dim, station))
            .and_then(|l| l.iter_mut().find(|p| p.pos == pos))
        {
            p.buffer = items;
            if !loaded {
                p.primed = true;
            }
        }
    });
    DIRTY.set(true);
}

fn is_package(lv: &ServerLevel<'_>, s: &ItemStack) -> bool {
    package::is_package(&body::item_id(&*lv.host, s))
}

/// `insertItemStacked` into `slots` (one package a slot here: packages don't stack): what is
/// left.
fn insert_stacked(slots: &mut [Option<ItemStack>], stack: ItemStack) -> Option<ItemStack> {
    let empty = slots.iter_mut().find(|s| s.is_none())?;
    *empty = Some(stack);
    None
}

/// `GlobalStation.runMailTransfer` for the train `train` (its `cars`) at `station`: each
/// carriage takes the postboxes' packages not for them, then gives its packages to the first
/// postbox whose address takes each. The postbox at a loaded place puts out its particles.
fn mail_transfer(lv: &mut ServerLevel<'_>, station: [i32; 3], train: u32, cars: usize) {
    let dim = lv.dim;
    let Some(ports) = PORTS.with_borrow(|m| m.get(&(dim, station)).cloned()) else {
        return;
    };
    let mut touched: Vec<[i32; 3]> = Vec::new();
    for car in 0..cars {
        let key = crate::glue::trains::cargo_key(train, car);
        let mut storage = crate::glue::contraptions::actors::lent_back(dim, key);
        if storage.is_empty() {
            crate::glue::contraptions::actors::lend_storage(dim, key, storage);
            continue;
        }
        // Import from the station.
        for port in &ports {
            let (mut slots, loaded) = slots_of(dim, port);
            let mut changed = false;
            for slot in slots.iter_mut() {
                let Some(stack) = slot.clone() else { continue };
                if !is_package(lv, &stack)
                    || package::match_address(&body::address(&stack), &port.address)
                {
                    continue;
                }
                let mut put = false;
                for (_, s) in storage.iter_mut() {
                    if let Some(free) = s.iter_mut().find(|x| x.is_none()) {
                        *free = Some(stack.clone());
                        put = true;
                        break;
                    }
                }
                if put {
                    *slot = None;
                    changed = true;
                }
            }
            if changed {
                put_slots(dim, station, port.pos, slots, loaded);
                if loaded {
                    touched.push(port.pos);
                }
            }
        }
        // Export to the station.
        for (_, s) in storage.iter_mut() {
            for slot in s.iter_mut() {
                let Some(stack) = slot.clone() else { continue };
                if !is_package(lv, &stack) {
                    continue;
                }
                let address = body::address(&stack);
                for port in &ports {
                    if !package::match_address(&address, &port.address) {
                        continue;
                    }
                    // Its slots as they are now (an earlier package may have gone in).
                    let now = PORTS.with_borrow(|m| {
                        m.get(&(dim, station))
                            .and_then(|l| l.iter().find(|p| p.pos == port.pos).cloned())
                    });
                    let Some(now) = now else { continue };
                    let (mut slots, loaded) = slots_of(dim, &now);
                    if insert_stacked(&mut slots, stack.clone()).is_some() {
                        continue;
                    }
                    *slot = None;
                    put_slots(dim, station, port.pos, slots, loaded);
                    if loaded {
                        touched.push(port.pos);
                    }
                    break;
                }
            }
        }
        crate::glue::contraptions::actors::lend_storage(dim, key, storage);
    }
    touched.sort_unstable();
    touched.dedup();
    for pos in touched {
        spawn_particles(lv, pos);
    }
}

/// `PostboxBlockEntity.spawnParticles`: a few happy villager sparks over it.
fn spawn_particles(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some(&key) = super::HAPPY.get() else {
        return;
    };
    let options = mod_api::ParticleOptions::simple(key);
    let at = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.5,
        f64::from(pos[2]) + 0.5,
    ];
    let _ = lv
        .level
        .send_particles(&mut *lv.host, &options, None, at, [0.3; 3], 0.0, 5);
}

/// The level's tick: the joins load with the overworld; each train standing at a station with
/// postboxes trades with them on arrival and every 21 ticks after (`ticksSinceLastMailTransfer
/// > 20`); every second the loaded postboxes' buffers follow them.
pub(super) fn level_tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    if lv.dimension == "minecraft:overworld" && !LOADED.get() {
        LOADED.set(true);
        load(lv);
    }
    if PORTS.with_borrow(|m| m.keys().any(|k| k.0 == dim)) {
        let standing: Vec<(u32, usize, [i32; 3])> = with_world(|w| {
            w.dims[dim]
                .railway
                .trains
                .iter()
                .filter_map(|t| Some((t.id, t.cars.len(), t.train.at_station?)))
                .collect()
        })
        .unwrap_or_default();
        AT.with_borrow_mut(|a| {
            a.retain(|k, _| k.0 != dim || standing.iter().any(|s| s.0 == k.1));
        });
        for (train, cars, station) in standing {
            let due = AT.with_borrow_mut(|a| {
                let e = a.entry((dim, train)).or_insert((station, u32::MAX));
                if e.0 != station || e.1 == u32::MAX {
                    // `arriveAt`.
                    *e = (station, 0);
                    return true;
                }
                e.1 += 1;
                if e.1 > 20 {
                    e.1 = 0;
                    return true;
                }
                false
            });
            if due {
                mail_transfer(lv, station, train, cars);
            }
        }
        if lv.game_time() % 20 == 0 {
            let loaded: Vec<([i32; 3], Vec<Option<ItemStack>>)> = PORTS.with_borrow(|m| {
                m.iter()
                    .filter(|(k, _)| k.0 == dim)
                    .flat_map(|(_, l)| l.iter().map(|p| p.pos))
                    .filter_map(|pos| {
                        with(dim, |pk| {
                            let port = pk.ports.get(&pos)?;
                            Some((
                                pos,
                                port.items
                                    .iter()
                                    .map(|s| s.as_ref().map(|b| b.stack.clone()))
                                    .collect(),
                            ))
                        })
                    })
                    .collect()
            });
            for (pos, items) in loaded {
                save_buffer(dim, pos, &items);
            }
        }
    }
    if lv.dimension == "minecraft:overworld" && DIRTY.get() {
        save(lv);
    }
}

/// The stations a package instruction may go to (`DeliverPackagesInstruction.start`,
/// `FetchPackagesInstruction.start`): for a delivery (`fetch` None), those with a postbox whose
/// address takes a package in `aboard`; for a retrieval, those whose postboxes hold a package to
/// send (not for the postbox's own address) whose address matches the filter (blank: any).
pub(crate) fn package_stations(
    lv: &ServerLevel<'_>,
    fetch: Option<&str>,
    aboard: &[ItemStack],
) -> Vec<[i32; 3]> {
    let dim = lv.dim;
    let ports: Vec<([i32; 3], StationPort)> = PORTS.with_borrow(|m| {
        m.iter()
            .filter(|(k, _)| k.0 == dim)
            .flat_map(|(k, l)| l.iter().map(|p| (k.1, p.clone())))
            .collect()
    });
    let stations: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].railway.stations.keys().copied().collect()).unwrap_or_default();
    let mut out: Vec<[i32; 3]> = Vec::new();
    for (station, port) in ports {
        if !stations.contains(&station) || out.contains(&station) {
            continue;
        }
        let found = match fetch {
            None => aboard.iter().any(|s| {
                is_package(lv, s) && package::match_address(&body::address(s), &port.address)
            }),
            Some(filter) => {
                let filter = if filter.trim().is_empty() {
                    "*"
                } else {
                    filter
                };
                slots_of(dim, &port).0.iter().flatten().any(|s| {
                    let address = body::address(s);
                    is_package(lv, s)
                        && !package::match_address(&address, &port.address)
                        && glob(filter, &address)
                })
            }
        };
        if found {
            out.push(station);
        }
    }
    out
}

/// `Glob.toRegexPattern(filter, "")` matched against `text`: `*` any run, `?` any one
/// character, the rest literal, the whole text.
fn glob(filter: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => (0..=t.len()).any(|i| go(rest, &t[i..])),
            Some(('?', rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((c, rest)) => t.first() == Some(c) && go(rest, &t[1..]),
        }
    }
    let p: Vec<char> = filter.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t)
}

fn stack_tag(lv: &ServerLevel<'_>, s: &ItemStack) -> Option<Tag> {
    s.to_nbt(&*lv.host).map(Tag::Compound)
}

fn save(lv: &mut ServerLevel<'_>) {
    let list: Vec<Tag> = PORTS.with_borrow(|m| {
        m.iter()
            .flat_map(|(k, l)| l.iter().map(move |p| (k, p)))
            .map(|((dim, station), p)| {
                let items: Vec<Tag> = p
                    .buffer
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| {
                        let Tag::Compound(mut c) = stack_tag(lv, s.as_ref()?)? else {
                            return None;
                        };
                        c.insert("Slot".into(), Tag::Int(i as i32));
                        Some(Tag::Compound(c))
                    })
                    .collect();
                let level = crate::glue::world::level_id(*dim).unwrap_or_default();
                Tag::compound([
                    ("Level", Tag::String(level.as_str().into())),
                    ("Station", Tag::IntArray(station.to_vec())),
                    ("Pos", Tag::IntArray(p.pos.to_vec())),
                    ("Address", Tag::String(p.address.as_str().into())),
                    ("Primed", Tag::Byte(i8::from(p.primed))),
                    ("OfflineBuffer", Tag::List(items)),
                ])
            })
            .collect()
    });
    let data = Compound::from([("Ports".into(), Tag::List(list))]);
    match SAVED.set(&mut *lv.host, Holder::Level, data) {
        Ok(()) => DIRTY.set(false),
        Err(e) => crate::create_log!("[create] saving the station ports: {e:?}"),
    }
}

fn load(lv: &mut ServerLevel<'_>) {
    let data = match SAVED.get_existing(&mut *lv.host, Holder::Level) {
        Ok(Some(d)) => d,
        _ => return,
    };
    let Some(Tag::List(list)) = data.get(&"Ports".into()) else {
        return;
    };
    let ints = |c: &Compound, k: &str| -> Option<[i32; 3]> {
        match c.get(&k.into()) {
            Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
            _ => None,
        }
    };
    let host: &dyn mod_api::Host = &*lv.host;
    PORTS.with_borrow_mut(|m| {
        for t in list {
            let Tag::Compound(c) = t else { continue };
            let level = match c.get(&"Level".into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => continue,
            };
            let Some(dim) = crate::glue::world::level_index(&level) else {
                continue;
            };
            let (Some(station), Some(pos)) = (ints(c, "Station"), ints(c, "Pos")) else {
                continue;
            };
            let mut buffer = vec![None; SLOTS];
            if let Some(Tag::List(items)) = c.get(&"OfflineBuffer".into()) {
                for i in items {
                    let Tag::Compound(i) = i else { continue };
                    let slot = match i.get(&"Slot".into()) {
                        Some(Tag::Int(v)) => *v as usize,
                        _ => continue,
                    };
                    if let (Some(b), Some(s)) = (buffer.get_mut(slot), ItemStack::from_nbt(host, i))
                    {
                        *b = Some(s);
                    }
                }
            }
            let entry = m.entry((dim, station)).or_default();
            entry.retain(|p| p.pos != pos);
            entry.push(StationPort {
                pos,
                address: match c.get(&"Address".into()) {
                    Some(Tag::String(s)) => s.to_string_lossy(),
                    _ => String::new(),
                },
                buffer,
                primed: matches!(c.get(&"Primed".into()), Some(Tag::Byte(1))),
            });
        }
    });
}
