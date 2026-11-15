//! The placard's block entity (`PlacardBlockEntity`): the item it shows reaches its clients in
//! its data (`Item`), and its renderer (`PlacardRenderer`) draws it on the placard's face.

use super::dir_named;
use create_engine::decor;
use create_engine::kinetics::Dir;
use mod_api::{
    BlockEntityPart, BlockEntitySubmitEvent, BlockEntityUpdateTagEvent, BlockPart, ClientHost,
    Compound, HostBlockStates, ItemDisplayContext, ItemStack, Registrar, Tag,
};

/// The placard's block entity type.
pub const TYPE: &str = "create:placard";

/// The key of the shown item in the placard's block entity data (an item stack compound; empty
/// for none).
pub(super) const ITEM: &str = "Item";

pub(super) fn register(r: &mut dyn Registrar) {
    let kind = r
        .block_entity_type(TYPE)
        .blocks(&["create:placard"])
        .build();
    if let Err(e) = r.block_entity_part(&kind, PlacardEntity) {
        crate::create_log!("[create] the placard block entity's part was refused: {e:?}");
    }
}

struct PlacardEntity;

#[mod_api::horizon_part]
impl BlockPart for PlacardEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for PlacardEntity {
    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        let item = event.entity.nbt.get(&ITEM.into())?.clone();
        Some(Compound::from([(ITEM.into(), item)]))
    }

    /// `PlacardRenderer`: the item turned to the placard's face and facing, 4.5 px out, a block
    /// item at half size, others at three eighths (`FIXED` display).
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(Tag::Compound(item)) = event.entity.nbt.get(&ITEM.into()) else {
            return;
        };
        if item.is_empty() {
            return;
        }
        let Some(stack) = ItemStack::from_nbt(host, item) else {
            return;
        };
        let facts = host.block_states();
        let prop = |k: &str| {
            event
                .state
                .property(&facts, k)
                .ok()
                .flatten()
                .map(str::to_owned)
        };
        let face = prop("face").unwrap_or_default();
        let facing = prop("facing")
            .as_deref()
            .and_then(dir_named)
            .unwrap_or(Dir::North);
        // `PlacardRenderer` halves an item whose model is 3D (`isGui3d`); no API reads that,
        // so an item that places a block stands for it.
        let block_item = host.item_block(stack.item()).is_some();
        let ceiling = face == "ceiling";
        let yaw = if ceiling { 180.0 } else { 0.0 } + 180.0 + decor::horizontal_angle(facing);
        let pitch = match face.as_str() {
            "ceiling" => -90.0,
            "floor" => 90.0,
            _ => 0.0,
        };
        let scale = if block_item { 0.5 } else { 0.375 };
        let draw = &mut *event.draw;
        // `decor::placard_pose`, step by step.
        draw.translate([0.5; 3]);
        draw.rotate_about([0.0, 1.0, 0.0], yaw, [0.0; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], pitch, [0.0; 3]);
        draw.translate([0.0, 0.0, 4.5 / 16.0]);
        draw.scale([scale; 3]);
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
        draw.keep();
    }
}

/// A placard riding a moving structure: its item as it was taken (`PlacardRenderer`).
pub(super) fn draw_carried(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    PlacardEntity.block_entity_submit(host, event);
}
