//! Create's own block entity fields (`SmartBlockEntity.write` and `read`) in each entity's saved
//! data, through `saveAdditional` and `loadAdditional`, so a chunk save, `/data`, loot and
//! pick-block-with-data all see them.
//!
//! A subsystem writes the fields of its block entities in a column
//! ([`Subsystem::save_entities`]) when the column is published; what it wrote is kept here by
//! entity and given to the entity's `saveAdditional`. The save hook cannot reach the item
//! registry a stack's NBT needs, so the fields are made at publication, which is also when a
//! changed entity is marked for its chunk's next save (`setChanged`). Saved data an entity loads
//! is given to every subsystem ([`Subsystem::load_entity`]) once its column's legacy data is in,
//! as Create's `read` runs before the block entity first ticks.

use super::SUBSYSTEMS;
use super::server::ServerLevel;
use super::states;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use mod_api::{
    BlockEntityLoadAdditionalEvent, BlockEntityPart, BlockEntitySaveAdditionalEvent,
    BlockEntityTypeKey, BlockPart, Compound, LevelHost, LevelRef, Registrar, Tag,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

/// What the subsystems write for the block entities of one column, by position.
pub type Entities = BTreeMap<[i32; 3], Compound>;

/// Marks data saved with the fields: what an older version put under the same keys (a drawn
/// filter's id, say) is not read as them, so its column's data stands.
const MARKER: &str = "create:fields";

/// A block entity's fields as last published or loaded, and the block they were written for.
#[derive(Clone, Debug, PartialEq)]
struct Record {
    block: Block,
    fields: Compound,
}

thread_local! {
    static RECORDS: RefCell<HashMap<(LevelRef, [i32; 3]), Record>> = RefCell::new(HashMap::new());
    /// Data block entities loaded that the subsystems have not read yet.
    static PENDING: RefCell<Vec<(LevelRef, [i32; 3], Record)>> = const { RefCell::new(Vec::new()) };
}

/// Adds the saved-fields part to block entity type `kind`: every type of Create's has it.
pub fn attach(r: &mut dyn Registrar, kind: &BlockEntityTypeKey) {
    if let Err(e) = r.block_entity_part(kind, SavedFields) {
        crate::create_log!("[create] the saved-fields part was refused: {e:?}");
    }
}

fn keys<'a>(
    systems: &'a [&'a dyn Subsystem],
    block: Block,
) -> impl Iterator<Item = &'static str> + 'a {
    systems
        .iter()
        .flat_map(move |s| s.saved_keys(block).iter().copied())
        .chain([MARKER])
}

fn own_fields(systems: &[&dyn Subsystem], block: Block, tag: &Compound) -> Compound {
    keys(systems, block)
        .filter_map(|k| Some((k.into(), tag.get(&k.into())?.clone())))
        .collect()
}

struct SavedFields;

#[mod_api::sunrise_part]
impl BlockPart for SavedFields {}

#[mod_api::sunrise_part]
impl BlockEntityPart for SavedFields {
    fn block_entity_save_additional(
        &self,
        _level: &dyn LevelHost,
        event: &mut BlockEntitySaveAdditionalEvent<'_>,
    ) {
        write_record(SUBSYSTEMS, event.level, event.pos, event.tag);
    }

    fn block_entity_load_additional(
        &self,
        level: &dyn LevelHost,
        event: &mut BlockEntityLoadAdditionalEvent<'_>,
    ) {
        let Some(block) = level
            .block_state(event.pos)
            .and_then(|s| states::level_block(level, s))
        else {
            return;
        };
        let known = RECORDS.with_borrow(|r| r.contains_key(&(event.level, event.pos)));
        if !known && !event.tag.contains_key(&MARKER.into()) {
            return;
        }
        let fields = own_fields(SUBSYSTEMS, block, event.tag);
        PENDING.with_borrow_mut(|p| {
            p.retain(|(l, q, _)| (*l, *q) != (event.level, event.pos));
            p.push((event.level, event.pos, Record { block, fields }));
        });
    }
}

/// `saveAdditional`: the entity's record replaces its block's saved keys, unless data it loaded
/// is still to be read (the tag is that data then).
fn write_record(systems: &[&dyn Subsystem], level: LevelRef, pos: [i32; 3], tag: &mut Compound) {
    if PENDING.with_borrow(|p| p.iter().any(|(l, q, _)| (*l, *q) == (level, pos))) {
        return;
    }
    RECORDS.with_borrow(|r| {
        let Some(record) = r.get(&(level, pos)) else {
            return;
        };
        for k in keys(systems, record.block) {
            tag.remove(&k.into());
        }
        tag.extend(record.fields.iter().map(|(k, v)| (k.clone(), v.clone())));
    });
}

/// The columns of `lv` holding loaded data still to be read.
pub fn pending_columns(lv: &ServerLevel<'_>) -> Vec<[i32; 2]> {
    let mut columns: Vec<[i32; 2]> = PENDING.with_borrow(|p| {
        p.iter()
            .filter(|(l, _, _)| *l == lv.level)
            .map(|(_, q, _)| [q[0] >> 4, q[2] >> 4])
            .collect()
    });
    columns.sort_unstable();
    columns.dedup();
    columns
}

/// `SmartBlockEntity.read` for each block entity of column `chunk` of `lv` that loaded data.
pub fn load(lv: &mut ServerLevel<'_>, chunk: [i32; 2]) {
    let level = lv.level;
    let loaded: Vec<([i32; 3], Record)> = PENDING.with_borrow_mut(|p| {
        let (mine, rest) = std::mem::take(p)
            .into_iter()
            .partition(|(l, q, _)| *l == level && [q[0] >> 4, q[2] >> 4] == chunk);
        *p = rest;
        mine.into_iter().map(|(_, q, r)| (q, r)).collect()
    });
    for (pos, record) in loaded {
        for s in SUBSYSTEMS {
            s.load_entity(lv, pos, record.block, &record.fields);
        }
        RECORDS.with_borrow_mut(|r| r.insert((level, pos), record));
    }
}

/// `SmartBlockEntity.write` for column `chunk` of `lv`: each block entity whose fields changed
/// keeps the new ones and is marked for its chunk's next save.
pub fn save(lv: &mut ServerLevel<'_>, chunk: [i32; 2]) {
    let mut fresh = Entities::new();
    for s in SUBSYSTEMS {
        s.save_entities(lv, chunk, &mut fresh);
    }
    let fresh: Vec<([i32; 3], Record)> = fresh
        .into_iter()
        .filter_map(|(pos, mut fields)| {
            fields.insert(MARKER.into(), Tag::Byte(1));
            let id = lv.block(pos);
            let block = with_world(|w| w.table.get(id).map(|(b, _)| b)).flatten()?;
            Some((pos, Record { block, fields }))
        })
        .collect();
    for pos in changed(lv.level, chunk, fresh) {
        let marked = lv
            .host
            .block_entity(lv.level, pos)
            .is_some_and(|e| e.set_changed(&mut *lv.host).is_ok());
        if !marked {
            RECORDS.with_borrow_mut(|r| r.remove(&(lv.level, pos)));
        }
    }
}

fn changed(level: LevelRef, chunk: [i32; 2], fresh: Vec<([i32; 3], Record)>) -> Vec<[i32; 3]> {
    let inside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] == chunk;
    let mut fresh: HashMap<[i32; 3], Record> = fresh.into_iter().collect();
    RECORDS.with_borrow_mut(|r| {
        let mut changed: Vec<[i32; 3]> = r
            .iter_mut()
            .filter(|((l, p), _)| *l == level && inside(p))
            .filter_map(|((_, p), old)| {
                let new = fresh.remove(p).unwrap_or(Record {
                    block: old.block,
                    fields: Compound::new(),
                });
                (*old != new).then(|| {
                    *old = new;
                    *p
                })
            })
            .collect();
        for (p, new) in fresh {
            if !new.fields.is_empty() {
                r.insert((level, p), new);
                changed.push(p);
            }
        }
        changed.sort_unstable();
        changed
    })
}

/// Splits column data whose lists hold compounds of block entities, each with its position under
/// `pos_key`, into the entities' fields: for each `(list, key)` of `names`, the entries of `list`
/// at a position, without it, are the list under `key` of the entity there.
pub fn split(lists: &Compound, pos_key: &str, names: &[(&str, &str)], out: &mut Entities) {
    for (list, key) in names {
        let Some(Tag::List(entries)) = lists.get(&(*list).into()) else {
            continue;
        };
        for entry in entries {
            let Tag::Compound(c) = entry else {
                continue;
            };
            let mut c = c.clone();
            let Some(Tag::IntArray(p)) = c.remove(&pos_key.into()) else {
                continue;
            };
            let Ok(pos) = <[i32; 3]>::try_from(p.as_slice()) else {
                continue;
            };
            let fields = out.entry(pos).or_default();
            if let Tag::List(l) = fields
                .entry((*key).into())
                .or_insert_with(|| Tag::List(Vec::new()))
            {
                l.push(Tag::Compound(c));
            }
        }
    }
}

/// What [`split`] took from column data for the block entity at `pos`, as column data again
/// (its entries at `pos`), for the column's reader.
pub fn join(tag: &Compound, pos: [i32; 3], pos_key: &str, names: &[(&str, &str)]) -> Compound {
    names
        .iter()
        .filter_map(|(list, key)| {
            let Tag::List(entries) = tag.get(&(*key).into())? else {
                return None;
            };
            let entries = entries
                .iter()
                .filter_map(|e| {
                    let Tag::Compound(c) = e else { return None };
                    let mut c = c.clone();
                    c.insert(pos_key.into(), Tag::IntArray(pos.to_vec()));
                    Some(Tag::Compound(c))
                })
                .collect();
            Some(((*list).into(), Tag::List(entries)))
        })
        .collect()
}

/// Column `chunk` of `level` left: its block entities were saved with it.
pub fn column_unloaded(level: LevelRef, chunk: [i32; 2]) {
    let outside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] != chunk;
    RECORDS.with_borrow_mut(|r| r.retain(|(l, p), _| *l != level || outside(p)));
    PENDING.with_borrow_mut(|p| p.retain(|(l, q, _)| *l != level || outside(q)));
}

/// The world closed.
pub fn clear() {
    RECORDS.with_borrow_mut(HashMap::clear);
    PENDING.with_borrow_mut(Vec::clear);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_fields_never_share_a_key_with_what_clients_are_sent() {
        let synced: std::collections::HashSet<&str> = SUBSYSTEMS
            .iter()
            .flat_map(|s| s.synced_keys().iter().copied())
            .collect();
        for block in Block::ALL {
            let valve = create_engine::dye::is_valve_handle(block);
            for key in keys(SUBSYSTEMS, block) {
                let handle_only = key == super::super::sources::handles::SCROLL_VALUE;
                if handle_only && !valve {
                    continue;
                }
                assert!(!synced.contains(key), "{block:?} saves synced key {key}");
            }
        }
    }
}
