//! Table cloths' hanging corners (`TableClothModel`): each side's quads carry a corner (south:
//! south west, west: north west, north: north east, east: south east), drawn unless that side
//! or the next one clockwise is culled. Create's model reads the neighbours as the chunk is
//! built; a terrain model here can't (`API_GAPS.md`), so the server works out which sides
//! are culled when a cloth is placed or a neighbour changes (`Cull`, in its block entity's
//! data) and the block entity draws the corners (`create:part_table_cloth_<kind>`). That is
//! the cloths' own type (`create:table_cloth`, `stock::shop`), whose update tag carries `Cull`.
//!
//! A side counts as culled (`Block.shouldRenderFace` false) beside another table cloth or a
//! carpet (the same face) and beside a block whose face toward the cloth is sturdy (a full,
//! solid face).

use super::super::server::ServerLevel;
use super::super::states;
use super::{add, face_of};
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{
    BlockEntityPart, BlockEntitySubmitEvent, BlockPart, ClientHost, Compound, SupportType, Tag,
};

/// The culled sides of a table cloth in its block entity's data: a bit per side, south, west,
/// north, east (`Direction.get2DDataValue`).
pub const CULL: &str = "Cull";

/// The horizontal sides by `get2DDataValue`, with the part's facing for each.
const SIDES: [(Dir, &str); 4] = [
    (Dir::South, "south"),
    (Dir::West, "west"),
    (Dir::North, "north"),
    (Dir::East, "east"),
];

pub fn is_cloth(block: Block) -> bool {
    block.name().ends_with("_table_cloth")
}

/// The sides of the cloth at `pos` the neighbours cull, into its block entity when they
/// changed.
pub(super) fn check(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    if !lv
        .block_name(pos)
        .is_some_and(|n| n.starts_with("create:") && n.ends_with("_table_cloth"))
    {
        return;
    }
    let mut mask = 0i8;
    for (i, (d, _)) in SIDES.iter().enumerate() {
        let p = add(pos, *d);
        let same = lv
            .block_name(p)
            .is_some_and(|n| n.ends_with("_table_cloth") || n.ends_with("_carpet"));
        let full =
            lv.level
                .block_face_sturdy(&*lv.host, p, face_of(d.opposite()), SupportType::Full)
                == Some(true);
        if same || full {
            mask |= 1 << i;
        }
    }
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let had = entity
        .data(&*lv.host)
        .and_then(|d| match d.get(&CULL.into()) {
            Some(Tag::Byte(b)) => Some(*b),
            _ => None,
        });
    if had == Some(mask) {
        return;
    }
    let data = Compound::from([(CULL.into(), Tag::Byte(mask))]);
    let _ = entity.merge_data(&mut *lv.host, data);
}

pub(super) struct ClothDraw;

#[mod_api::horizon_part]
impl BlockPart for ClothDraw {}

#[mod_api::horizon_part]
impl BlockEntityPart for ClothDraw {
    fn block_entity_submit(&self, _host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(kind) = event
            .block
            .strip_prefix("create:")
            .and_then(|n| n.strip_suffix("_table_cloth"))
        else {
            return;
        };
        let Some(part) = states::partial(&format!("part_table_cloth_{kind}")) else {
            return;
        };
        let mask = match event.entity.nbt.get(&CULL.into()) {
            Some(Tag::Byte(b)) => *b,
            _ => 0,
        };
        let culled = |i: usize| mask & (1 << (i % 4)) != 0;
        for (i, (_, facing)) in SIDES.iter().enumerate() {
            // The side's own face and the next one clockwise (`getClockWise`).
            if !culled(i) && !culled(i + 1) {
                event.draw.block(part, &[("facing", facing)]);
            }
        }
        event.draw.keep();
    }
}
