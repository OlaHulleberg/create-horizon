//! Publish the contents of a kept block-entity drawing. Create's client packets
//! (`DepotBehaviour.write`, `BeltInventory.write`, FlapDisplayBlockEntity and
//! LinkBehaviour) carry state; repeating the same state is not a new revision.
use super::server::ServerLevel;
use mod_api::{Compound, Tag};

pub(crate) const SHOWN: &str = "Shown";

/// Exact contents already encoded by the subsystem's client-packet codec.
/// Keep packet boundaries, so comparison never relies on a hash or serial counter.
pub(crate) fn snapshot(messages: impl IntoIterator<Item = Vec<u8>>) -> Tag {
    Tag::List(messages.into_iter().map(Tag::ByteArray).collect())
}

/// Old integer revisions naturally migrate on the first publication.
pub(crate) fn update(previous: Option<&Tag>, contents: Tag) -> Option<Tag> {
    (previous != Some(&contents)).then_some(contents)
}

/// Publish this entity's drawn contents only when they changed. Data writes can
/// themselves cause contents notifications in the SDK, so this must converge.
/// Call after sending the client packet: its contents must reach the ordered
/// receiver before the block-entity update invalidates the kept drawing.
pub(crate) fn publish(lv: &mut ServerLevel<'_>, pos: [i32; 3], contents: Tag) {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let Some(data) = entity.data(&*lv.host) else {
        return;
    };
    let changed = update(data.get(&SHOWN.into()), contents);
    if let Some(contents) = changed {
        let _ = entity.merge_data(&mut *lv.host, Compound::from([(SHOWN.into(), contents)]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use create_engine::{belt, kinetics::Dir};

    #[test]
    fn belt_redraw_notifications_converge_and_inventory_changes_publish() {
        let mut contents = belt::Contents {
            controller: [60, 100, 0],
            segment: belt::Segment {
                facing: Dir::South,
                slope: belt::Slope::Horizontal,
                part: belt::Part::Start,
            },
            length: 4,
            ending: belt::Ending::Eject,
            items: belt::Inventory::default(),
        };
        contents.items = belt::Inventory::from_saved(
            vec![belt::Item::new("minecraft:diamond", 2, 172, false)],
            false,
        );
        let mut shown = Tag::Int(9914);
        let mut writes = 0;
        for _ in 0..2000 {
            if let Some(next) = update(Some(&shown), snapshot(belt::encode(&[contents.clone()]))) {
                shown = next;
                writes += 1;
            }
        }
        assert_eq!(
            writes, 1,
            "the contents notification caused by publication must settle"
        );
        contents.items.items.clear();
        contents.items = belt::Inventory::from_saved(
            vec![belt::Item::new("minecraft:emerald", 3, 172, false)],
            false,
        );
        shown = update(Some(&shown), snapshot(belt::encode(&[contents.clone()])))
            .expect("replacement redraws");
        contents.items.items.clear();
        assert!(
            update(Some(&shown), snapshot(belt::encode(&[contents]))).is_some(),
            "emptying redraws"
        );
    }
}
