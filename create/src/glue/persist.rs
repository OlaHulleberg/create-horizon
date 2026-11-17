//! What Create keeps for each column of a level: one chunk attachment holding a compound, with a
//! key per subsystem (`create`, `create_lifts`...), as Create keeps its block entities' data.
//!
//! The game saves a chunk's attachments with it, and saves the chunk before it unloads. There
//! is no moment just before a chunk is saved, so a column whose state changed is written into
//! its attachment at the end of the level tick that changed it and when an interaction
//! is accepted. A column that did not change is not written.

use super::server::ServerLevel;
use super::subsystem::Subsystem;
use mod_api::{AttachmentCodec, AttachmentType, Compound, Holder};
use std::cell::RefCell;
use std::collections::BTreeSet;

/// The column's data. Saved under `horizon:attachments` / `create` / `column`.
pub static COLUMN: AttachmentType<Compound> = AttachmentType {
    namespace: "create",
    name: "column",
    default: Compound::new,
    codec: Some(AttachmentCodec {
        encode: |data| Ok(data.clone()),
        decode: |data| Ok(data.clone()),
    }),
    copy_on_death: false,
    sync: false,
};

thread_local! {
    /// Columns whose data changed since they were last written, by level index.
    static UNSAVED: RefCell<BTreeSet<(usize, [i32; 2])>> = const { RefCell::new(BTreeSet::new()) };
}

/// Marks column `chunk` of level `dim` to be written at the end of this tick.
pub fn mark_unsaved(dim: usize, chunk: [i32; 2]) {
    UNSAVED.with_borrow_mut(|u| {
        u.insert((dim, chunk));
    });
}

/// Marks the column holding `pos`.
pub fn mark_pos_unsaved(dim: usize, pos: [i32; 3]) {
    mark_unsaved(dim, [pos[0] >> 4, pos[2] >> 4]);
}

/// Forgets every mark (the world closed).
pub fn clear() {
    UNSAVED.with_borrow_mut(BTreeSet::clear);
}

/// The saved data of column `chunk` of `lv`, read when it loads. Empty for a column Create
/// never wrote.
pub fn read(lv: &mut ServerLevel<'_>, chunk: [i32; 2]) -> Compound {
    match COLUMN.get_existing(&mut *lv.host, Holder::Chunk(chunk)) {
        Ok(Some(data)) => data,
        Ok(None) => Compound::new(),
        Err(e) => {
            crate::create_log!("[create] column {chunk:?} of {}: {e:?}", lv.dimension);
            Compound::new()
        }
    }
}

/// Writes every marked column of `lv` that is still loaded, asking each subsystem for its part.
pub fn flush(lv: &mut ServerLevel<'_>, systems: &[&dyn Subsystem]) {
    let dim = lv.dim;
    let columns: Vec<[i32; 2]> = UNSAVED.with_borrow_mut(|u| {
        let mine: Vec<_> = u.iter().filter(|(d, _)| *d == dim).copied().collect();
        for key in &mine {
            u.remove(key);
        }
        mine.into_iter().map(|(_, c)| c).collect()
    });
    for chunk in columns {
        let loaded = lv
            .level
            .chunk_ref(&*lv.host, chunk)
            .is_some_and(|c| c.loaded(&*lv.host));
        if !loaded {
            continue;
        }
        let mut data = Compound::new();
        for s in systems {
            s.chunk_data_save(lv, chunk, &mut data);
        }
        let result = if data.is_empty() {
            COLUMN
                .remove(&mut *lv.host, Holder::Chunk(chunk))
                .map(|_| ())
        } else {
            COLUMN.set(&mut *lv.host, Holder::Chunk(chunk), data)
        };
        if let Err(e) = result {
            crate::create_log!(
                "[create] writing column {chunk:?} of {}: {e:?}",
                lv.dimension
            );
        }
    }
}

/// Complete Create's synchronous destroyBlock effects, then publish live records
/// and flush columns. Shared by accepted interactions and completed level ticks;
/// no entity tick or kinetic solve runs here.
pub(crate) fn publish(lv: &mut ServerLevel<'_>) {
    super::kinetics::drain_effects(lv);
    for (cx, cz) in super::world::with_world(|w| w.take_unsaved(lv.dim)).unwrap_or_default() {
        mark_unsaved(lv.dim, [cx, cz]);
    }
    for s in super::SUBSYSTEMS {
        s.write_entities(lv);
    }
    flush(lv, super::SUBSYSTEMS);
}
