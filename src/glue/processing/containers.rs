//! The item slots of the processing machines' inventories and the toolboxes', as hoppers,
//! comparators and breaking the block see them (`ModContainer`): the millstone, basin,
//! mechanical crafter and toolboxes; and what a change of their contents sets off
//! (`contents_changed`), as the legacy `block_entity_container` and `contents_changed` did for
//! them. The depot's, weighted ejector's, item vault's and chutes' are logistics', with their
//! `ITEMS` handlers.
//!
//! The slots are on the blocks' block entities: the `create:kinetic` type for all but the
//! toolboxes, which keep no other state and have their own type, `create:toolbox`.

use crate::glue::server::ServerLevel;
use crate::glue::states;
use crate::glue::world::with_world;
use create_engine::{Block, basin, toolbox};
use mod_api::{BlockPart, ContentsChangedEvent, ModContainer, Registrar, ServerHost};

/// The toolboxes' block entity type.
pub const TOOLBOX_TYPE: &str = "create:toolbox";

fn toolboxes() -> impl Iterator<Item = Block> {
    Block::ALL
        .iter()
        .copied()
        .filter(|&b| create_engine::dye::is_toolbox(b))
}

/// Declares the slots of each block with an inventory, the toolboxes' type (their menu's screen
/// is `blocks`'), and the contents-changed part of each.
pub fn register(r: &mut dyn Registrar) {
    for (block, container) in containers() {
        r.block_container(&states::key(block), container);
        r.block_part(states::key(block), ContentsChanged);
    }
    let names: Vec<String> = toolboxes()
        .map(|b| format!("create:{}", b.name()))
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    r.block_entity_type(TOOLBOX_TYPE).blocks(&names).build();
}

/// Every block's container, as the legacy glue declared it.
fn containers() -> Vec<(Block, ModContainer)> {
    let every = |slots: &[usize]| std::array::from_fn(|_| slots.to_vec());
    let mut out = Vec::new();
    // ToolboxInventory: 32 slots (8 compartments of 4 stacks), in and out through any face;
    // a four-row grid menu, whose last four slots are hidden.
    let stored: Vec<usize> = (0..toolbox::STORED).collect();
    for b in toolboxes() {
        out.push((
            b,
            ModContainer {
                slots: toolbox::SLOTS,
                faces: every(&stored),
                inputs: stored.clone(),
                outputs: stored.clone(),
                menu: Some(toolbox::MENU.into()),
            },
        ));
    }
    // MechanicalCrafterBlockEntity.Inventory: one item.
    out.push((
        Block::MechanicalCrafter,
        ModContainer {
            slots: 1,
            faces: every(&[0]),
            inputs: vec![0],
            outputs: vec![],
            menu: None,
        },
    ));
    // The basin's inventories: hoppers put into the nine inputs and take from the nine outputs.
    let all: Vec<usize> = (0..basin::SLOTS).collect();
    out.push((
        Block::Basin,
        ModContainer {
            slots: basin::SLOTS,
            faces: every(&all),
            inputs: basin::INPUTS.collect(),
            outputs: basin::OUTPUTS.collect(),
            menu: None,
        },
    ));
    // MillstoneInventoryHandler: hoppers put into the input (0) and take from the nine
    // outputs, through any face; no menu.
    let all: Vec<usize> = (0..10).collect();
    out.push((
        Block::Millstone,
        ModContainer {
            slots: 10,
            faces: every(&all),
            inputs: vec![0],
            outputs: (1..10).collect(),
            menu: None,
        },
    ));
    out
}

/// The part of every block with a container: a hopper (or anything moving items through the
/// container) put into or took from its slots.
struct ContentsChanged;

#[mod_api::horizon_part]
impl BlockPart for ContentsChanged {
    fn contents_changed(&self, host: &mut dyn ServerHost, event: &mut ContentsChangedEvent) {
        let Some(dimension) = event.level.dimension().id(&*host) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        contents_changed(&mut lv, pos);
    }
}

/// `contents_changed` (legacy): chutes asleep under this container look into it again, arms
/// using it as a point look again, and so do smart observers and threshold switches looking at
/// it; then each block's own: an item put into a crafter may start its chain, a basin's mixer
/// looks again.
fn contents_changed(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some((block, _)) = lv.info(pos) else {
        return;
    };
    let dim = lv.dim;
    with_world(|w| {
        w.container_changed(dim, pos);
        w.detectors_contents_changed(dim, pos);
        w.dims[dim].arms.touched(pos);
    });
    match block {
        // `insertItem` -> `checkCompletedRecipe`.
        Block::MechanicalCrafter => {
            with_world(|w| w.dims[dim].crafter_checks.push(pos));
        }
        Block::Basin => {
            with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
        }
        _ => {}
    }
}
