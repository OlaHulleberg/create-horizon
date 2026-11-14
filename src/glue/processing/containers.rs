//! The item slots of Create's blocks with inventories, as hoppers, comparators and breaking the
//! block see them (`ModContainer`): the millstone, depot, weighted ejector, basin, chute, smart
//! chute, item vault, mechanical crafter and toolboxes; the toolbox's menu and screen; and what
//! a change of their contents sets off (`contents_changed`).
//!
//! The slots are on the blocks' block entities: the `create:kinetic` type for all but the
//! toolboxes, which keep no other state and have their own type, `create:toolbox`.

use crate::glue::logistics::{random_angle, upright};
use crate::glue::server::ServerLevel;
use crate::glue::states;
use crate::glue::world::with_world;
use create_engine::{Block, basin, depot, toolbox, vault};
use mod_api::{
    BlockEntityPart, BlockPart, ContentsChangedEvent, Host, ModContainer, Registrar, Screen,
    ScreenEvent, ServerHost,
};

/// The toolboxes' block entity type.
pub const TOOLBOX_TYPE: &str = "create:toolbox";

fn toolboxes() -> impl Iterator<Item = Block> {
    Block::ALL
        .iter()
        .copied()
        .filter(|&b| create_engine::dye::is_toolbox(b))
}

/// Declares the slots of each block with an inventory, the toolboxes' type with their screen,
/// and the contents-changed part of each.
pub fn register(r: &mut dyn Registrar) {
    for (block, container) in containers() {
        r.block_container(&states::key(block), container);
        r.block_part(states::key(block), ContentsChanged);
    }
    let names: Vec<String> = toolboxes()
        .map(|b| format!("create:{}", b.name()))
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let kind = r.block_entity_type(TOOLBOX_TYPE).blocks(&names).build();
    if let Err(e) = r.block_entity_part(&kind, ToolboxEntity) {
        eprintln!("[create] the toolbox block entity's part was refused: {e:?}");
    }
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
    // DepotItemHandler (the ejector's `DepotBehaviour` too): in only to the held slot; out of
    // every slot; through any face.
    let all: Vec<usize> = (0..depot::SLOTS).collect();
    for b in [Block::Depot, Block::WeightedEjector] {
        out.push((
            b,
            ModContainer {
                slots: depot::SLOTS,
                faces: every(&all),
                inputs: vec![0],
                outputs: all.clone(),
                menu: None,
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
    // ItemVaultBlockEntity's ItemStackHandler (`vaultCapacity` stacks): in and out through
    // any face. Hoppers and comparators see this block's own slots; Create's funnels and
    // chutes the whole vault's.
    let all: Vec<usize> = (0..vault::SLOTS).collect();
    out.push((
        Block::ItemVault,
        ModContainer {
            slots: vault::SLOTS,
            faces: every(&all),
            inputs: all.clone(),
            outputs: all,
            menu: None,
        },
    ));
    // ChuteItemHandler of a smart chute: hoppers below take its stack; nothing is put in from
    // outside, as a hopper here could not ask its filter (`canAcceptItem`).
    out.push((
        Block::SmartChute,
        ModContainer {
            slots: 1,
            faces: every(&[0]),
            inputs: vec![],
            outputs: vec![0],
            menu: None,
        },
    ));
    // ChuteItemHandler: one stack.
    out.push((
        Block::Chute,
        ModContainer {
            slots: 1,
            faces: every(&[0]),
            inputs: vec![0],
            outputs: vec![0],
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

/// A toolbox's block entity: its screen shows its eight compartments as columns of four
/// stacks, the container's four slots past Create's 32 hidden (the server's menu leaves them
/// out too).
struct ToolboxEntity;

#[mod_api::horizon_part]
impl BlockPart for ToolboxEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for ToolboxEntity {
    fn screen(&self, _host: &dyn Host, event: &mut ScreenEvent<'_>) -> Option<Screen> {
        let block = event
            .block
            .strip_prefix("create:")
            .and_then(Block::from_name)?;
        if !create_engine::dye::is_toolbox(block) {
            return None;
        }
        Some(Screen {
            width: toolbox::SCREEN_WIDTH,
            height: toolbox::SCREEN_HEIGHT,
            slots: (0..toolbox::SLOTS).map(toolbox::slot_position).collect(),
            inventory_y: toolbox::INVENTORY_Y,
            inventory_x: toolbox::INVENTORY_X,
            ..Default::default()
        })
    }
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
/// using it as a point and an ejector it is look again, and so do smart observers and threshold
/// switches looking at it; then each block's own: a vault's comparators read the whole vault,
/// an item put into a crafter may start its chain, a chute given a stack has it fall from the
/// top, a basin's mixer looks again, and a depot's held stack arrives from above (`new
/// TransportedItemStack(stack)`, a quarter-less slide from the edge of nothing) or left.
fn contents_changed(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some((block, _)) = lv.info(pos) else {
        return;
    };
    let dim = lv.dim;
    with_world(|w| {
        w.container_changed(dim, pos);
        w.detectors_contents_changed(dim, pos);
        w.dims[dim].arms.touched(pos);
        let d = &mut w.dims[dim];
        if d.ejectors.contains_key(&pos) && !d.ejecting.contains(&pos) {
            d.ejecting.push(pos);
        }
    });
    match block {
        // `ItemVaultBlockEntity.updateComparators`: those beside its other blocks read again.
        Block::ItemVault => {
            with_world(|w| w.dims[dim].analog_checks.push(pos));
        }
        // `insertItem` -> `checkCompletedRecipe`.
        Block::MechanicalCrafter => {
            with_world(|w| w.dims[dim].crafter_checks.push(pos));
        }
        Block::Chute | Block::SmartChute => {
            with_world(|w| {
                w.dims[dim].chutes.entry(pos).or_insert(1.0);
            });
        }
        Block::Basin => {
            with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
        }
        _ => depot_contents_changed(lv, pos),
    }
}

/// A depot's (or an ejector's) held slot changed under a hopper.
fn depot_contents_changed(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    // Outer: the world is open; inner: a depot is there (its held stack's motion, if any).
    let Some(had) = with_world(|w| w.dims[dim].depots.get(&pos).copied()).flatten() else {
        return;
    };
    let held = lv.slot(pos, 0);
    let now = match (had, held) {
        (_, None) => None,
        (Some(h), Some(_)) => Some(h),
        (None, Some(s)) => {
            let item = super::stacks::id(&*lv.host, &s);
            let up = upright(&*lv.host, &item);
            Some(depot::Held {
                position: 0.0,
                ..depot::Held::new(random_angle(lv, pos), up)
            })
        }
    };
    with_world(|w| w.depot_put(dim, pos, now));
}
