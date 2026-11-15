//! The item hatch's block entity (`ItemHatchBlockEntity`): its filter reaches its clients in its
//! data (`Filter`, the filter's stack), and `FilteringRenderer` draws it on the hatch's slot
//! (`HatchFilterSlot`: turned to the hatch's facing and tipped 45° back).

use super::dir_named;
use create_engine::kinetics::Dir;
use mod_api::{
    BlockEntityPart, BlockEntitySubmitEvent, BlockEntityUpdateTagEvent, BlockPart, ClientHost,
    Compound, HostBlockStates, ItemDisplayContext, ItemStack, Registrar, Tag,
};

/// The hatch's block entity type.
pub const TYPE: &str = "create:item_hatch";

/// The key of the filter's stack in the hatch's block entity data (empty for none).
pub(super) const FILTER: &str = "Filter";

pub(super) fn register(r: &mut dyn Registrar) {
    let kind = r
        .block_entity_type(TYPE)
        .blocks(&["create:item_hatch"])
        .build();
    if let Err(e) = r.block_entity_part(&kind, HatchEntity) {
        crate::create_log!("[create] the item hatch block entity's part was refused: {e:?}");
    }
}

struct HatchEntity;

#[mod_api::horizon_part]
impl BlockPart for HatchEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for HatchEntity {
    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        let item = event.entity.nbt.get(&FILTER.into())?.clone();
        Some(Compound::from([(FILTER.into(), item)]))
    }

    /// `FilteringRenderer.renderOnBlockEntity` with `HatchFilterSlot`: at `voxelSpace(8, 5.15,
    /// 9.5)` turned by the hatch's `horizontalAngle` about y, turned the same way and 45° back,
    /// at 0.965 of the value box's scale (`ValueBoxRenderer.renderItemIntoValueBox`: a flat item
    /// at half that, pushed back .15).
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(Tag::Compound(item)) = event.entity.nbt.get(&FILTER.into()) else {
            return;
        };
        if item.is_empty() {
            return;
        }
        let Some(stack) = ItemStack::from_nbt(host, item) else {
            return;
        };
        let facts = host.block_states();
        let facing = event
            .state
            .property(&facts, "facing")
            .ok()
            .flatten()
            .and_then(dir_named)
            .unwrap_or(Dir::North);
        let angle = create_engine::factory::horizontal_angle(facing) as f32;
        let (s, c) = angle.to_radians().sin_cos();
        let v = [0.0, 5.15 / 16.0 - 0.5, 9.5 / 16.0 - 0.5];
        let at = [
            v[0] * c + v[2] * s + 0.5,
            v[1] + 0.5,
            v[2] * c - v[0] * s + 0.5,
        ];
        let solid = host.item_block(stack.item()).is_some();
        let scale = 0.5 * 0.965 * if solid { 1.0 + 1.0 / 64.0 } else { 0.5 };
        let draw = &mut *event.draw;
        draw.translate(at);
        draw.rotate_about([0.0, 1.0, 0.0], angle, [0.0; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], -45.0, [0.0; 3]);
        draw.scale([scale; 3]);
        if !solid {
            draw.translate([0.0, 0.0, -0.15]);
        }
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
        draw.keep();
    }
}
