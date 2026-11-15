//! Create's copycat panel and step (`content/decoration/copycat`); the rules are
//! `create_engine::copycat`'s. A copycat's material (a block state) and the item it consumed for
//! it (`CopycatBlockEntity`: `material`, `consumedItem`) are kept per level, saved with their
//! column as `create_copycat`. Its block moments are Parts on its blocks
//! (`parts.rs`: `useItemOn`, `getStateForPlacement`, `setPlacedBy` on the copycat items); its
//! block entity (`create:copycat`) carries what clients draw, the material cut to the copycat's
//! box (`CopycatModel`), the copycat base when it holds none. Nothing runs per tick: the level's
//! tick looks only when a copycat was placed, changed or broken, or its material changed.
//!
//! A copycat item clicked on its kind places the next one beside it (`CopycatPanelBlock` and
//! `CopycatStepBlock`'s placement helpers). A copycat sounds as its material (`sound.rs`).
//!
//! Left out, for want of API:
//! - The material's light and friction (`CopycatBlock.getLightEmission`, `getFriction`): the
//!   `light_emission` and `friction` parts could answer per copycat, but no API reads a state's
//!   light emission or friction (`API_GAPS.md`).
//! - `getAcceptedBlockState`'s class tests (`StairBlock`, `IronBarsBlock`, `TrapDoorBlock`):
//!   no API says which vanilla class a block is.
//!   They are read from vanilla's tags (`minecraft:stairs`, `minecraft:trapdoors`) and, for bars,
//!   a `_bars` id that is no pane; a block-class query would make them exact.
//!
//! A copycat's block model is not drawn (`render_shape` `ENTITYBLOCK_ANIMATED` in its
//! declaration): its block entity draws the material cut to its box, or the copycat base when it
//! holds none. The wrench takes the material off with its break particles and sound (rebuilt as
//! `levelEvent(2001)` shows them, `ServerLevel::destroy_effect`), the item it consumed going back to the player
//! outside creative (`onWrenched`), and a sneaking wrench does so before it picks the copycat up.

mod body;
mod parts;
mod sound;
pub(crate) use parts::helper_target;

use super::persist;
use super::server::ServerLevel;
use super::states;
use super::subsystem::Subsystem;
use super::world::level_index;
use create_engine::Block;
use create_engine::copycat::{self as cc, Shape, Special};
use create_engine::kinetics::Dir;
use mod_api::{
    BlockEntityTypeKey, BlockState, BreakBlockEvent, Command, CommandArgumentValue, Compound, Flow,
    HostBlockStates, ItemStack, RegisterCommandsEvent, Registrar, RegistrationContext,
    RunModCommandEvent, ServerHost, Tag, Text,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The key of a column's copycats in its saved data.
const KEY: &str = "create_copycat";

/// The block entity type of the copycat panel and step.
pub const TYPE: &str = "create:copycat";

/// The block entity data clients draw from: the material's state text (empty for the base) and
/// its `CopycatSpecialCases` (0 none, 1 bars, 2 trapdoor).
const MATERIAL: &str = "Material";
const SPECIAL: &str = "Special";

static KIND: OnceLock<BlockEntityTypeKey> = OnceLock::new();

/// A copycat's block entity (`CopycatBlockEntity`): its material (None: the copycat base) and
/// the item it consumed for it (`consumedItem`, one, kept with its components).
#[derive(Clone, Default)]
struct Copycat {
    material: Option<BlockState>,
    item: Option<ItemStack>,
}

#[derive(Default)]
struct Dim {
    copycats: HashMap<[i32; 3], Copycat>,
    /// Copycats placed, changed or broken: position, old and new engine state.
    changes: Vec<([i32; 3], u32, u32)>,
    /// Copycats whose block entity must show its material again.
    shown: Vec<[i32; 3]>,
}

thread_local! {
    static COPYCATS: RefCell<HashMap<usize, Dim>> = RefCell::new(HashMap::new());
}

fn with<R>(dim: usize, f: impl FnOnce(&mut Dim) -> R) -> R {
    COPYCATS.with_borrow_mut(|d| f(d.entry(dim).or_default()))
}

fn shown(dim: usize, pos: [i32; 3]) {
    with(dim, |d| d.shown.push(pos));
}

/// The copycat block `block` is, if it is one.
fn copycat_block(block: mod_api::BlockKey) -> Option<Block> {
    [Block::CopycatPanel, Block::CopycatStep]
        .into_iter()
        .find(|b| states::key(*b) == block)
}

fn is_copycat_id(id: u32) -> bool {
    states::state(id).is_some_and(|s| copycat_block(s.block()).is_some())
}

/// A copycat's shape from its block state (`facing`, a step's `half`).
fn shape_of(host: &dyn mod_api::Host, state: BlockState) -> Option<Shape> {
    let block = copycat_block(state.block())?;
    let facts = host.block_states();
    let prop = |k: &str| state.property(&facts, k).ok().flatten().map(str::to_owned);
    let facing = prop("facing").as_deref().and_then(cc::dir_named);
    Some(match block {
        Block::CopycatPanel => Shape::Panel(facing.unwrap_or(Dir::Up)),
        _ => Shape::Step {
            facing: facing.unwrap_or(Dir::South),
            top: prop("half").as_deref() == Some("top"),
        },
    })
}

fn center(pos: [i32; 3]) -> [f64; 3] {
    pos.map(|v| f64::from(v) + 0.5)
}

/// `state` with `key` set to `value`, when it has that property and allows the value.
fn with_prop(
    host: &dyn mod_api::Host,
    state: BlockState,
    key: &str,
    value: &str,
) -> Option<BlockState> {
    state.with(&host.block_states(), key, value).ok()
}

pub struct Copycats;

/// What a moving structure carries for a copycat (`Subsystem::carry_out`).
const CARRIED_MATERIAL: &str = "CopycatMaterial";
const CARRIED_ITEM: &str = "CopycatItem";

impl Subsystem for Copycats {
    fn draw_carried(
        &self,
        host: &dyn mod_api::ClientHost,
        block: Block,
        event: &mut mod_api::BlockEntitySubmitEvent<'_>,
    ) {
        if matches!(block, Block::CopycatPanel | Block::CopycatStep) {
            parts::draw_carried(host, event);
        }
    }

    /// A copycat's material and the item it took ride with it (`CopycatBlockEntity.write`), the
    /// material turned with the structure (`CopycatBlockEntity.transform`).
    fn carry_out(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], _b: Block, out: &mut Compound) {
        let Some(c) = with(lv.dim, |d| d.copycats.remove(&pos)) else {
            return;
        };
        let host = &*lv.host;
        if let Some(m) = c.material.and_then(|m| host.block_state_string(m)) {
            out.insert(CARRIED_MATERIAL.into(), Tag::String(m.as_str().into()));
        }
        if let Some(item) = c.item.and_then(|i| i.to_nbt(host)) {
            out.insert(CARRIED_ITEM.into(), Tag::Compound(item));
        }
    }

    fn carry_in(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        b: Block,
        data: &Compound,
        turn: create_engine::contraption::Transform,
    ) {
        if !matches!(b, Block::CopycatPanel | Block::CopycatStep) {
            return;
        }
        let host = &*lv.host;
        let material = match data.get(&CARRIED_MATERIAL.into()) {
            Some(Tag::String(m)) => host.block_state_id(&m.to_string_lossy()),
            _ => None,
        };
        let material = match (material, turn.quarter_axis()) {
            (Some(m), Some((quarter, axis))) => {
                Some(crate::glue::trains::turned_state(host, m, quarter, axis))
            }
            (m, _) => m,
        };
        let item = match data.get(&CARRIED_ITEM.into()) {
            Some(Tag::Compound(c)) => ItemStack::from_nbt(host, c),
            _ => None,
        };
        if material.is_none() && item.is_none() {
            return;
        }
        with(lv.dim, |d| {
            d.copycats.insert(pos, Copycat { material, item });
            d.shown.push(pos);
        });
    }

    /// The wrench on a copycat: `CopycatBlock.onWrenched`
    /// takes its material off and it never turns; `onSneakWrenched` takes it off before the
    /// wrench picks the block up.
    fn wrench_used(
        &self,
        lv: &mut ServerLevel<'_>,
        player: mod_api::PlayerRef,
        pos: [i32; 3],
        block: Block,
        _clicked: create_engine::kinetics::Dir,
        _point: [f64; 3],
        sneaking: bool,
    ) -> bool {
        if !matches!(block, Block::CopycatPanel | Block::CopycatStep) {
            return false;
        }
        parts::wrench(lv, pos, player);
        !sneaking
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        for block in [Block::CopycatPanel, Block::CopycatStep] {
            r.block_part(states::key(block), parts::CopycatBlock);
            r.block_part(states::key(block), sound::CopycatSound);
            let item = r.item_key(&format!("create:{}", block.name()));
            r.item_part(item, parts::CopycatItem);
        }
        let kind = r
            .block_entity_type(TYPE)
            .blocks(&["create:copycat_panel", "create:copycat_step"])
            .build();
        let _ = KIND.set(kind);
        if let Err(e) = r.block_entity_part(&kind, parts::CopycatEntity) {
            crate::create_log!("[create] the copycat block entity's part was refused: {e:?}");
        }
    }

    fn world_closed(&self) {
        COPYCATS.with_borrow_mut(HashMap::clear);
        sound::reset_server();
    }

    fn client_reset(&self) {
        sound::reset_client();
    }

    /// One state lookup for each Create block changed.
    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        if is_copycat_id(old) || is_copycat_id(new) {
            with(dim, |d| d.changes.push((pos, old, new)));
        }
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tick(lv);
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        load(lv, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        save(lv, chunk, data);
    }

    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        let outside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] != chunk;
        with(dim, |d| {
            d.copycats.retain(|p, _| outside(p));
            d.shown.retain(outside);
        });
    }

    /// `playerWillDestroy`: a creative player breaking a copycat keeps its item from dropping.
    fn break_block(&self, host: &mut dyn ServerHost, event: &mut BreakBlockEvent<'_>) -> Flow {
        if event.cancelled || copycat_block(event.state.block()).is_none() {
            return Flow::Continue;
        }
        let Some(player) = host.players().acting_player() else {
            return Flow::Continue;
        };
        if player.is_creative(&*host) != Some(true) {
            return Flow::Continue;
        }
        let dim = player
            .dimension(&*host)
            .and_then(|d| level_index(&d.to_string()));
        if let Some(dim) = dim {
            with(dim, |d| {
                if let Some(c) = d.copycats.get_mut(&event.pos) {
                    c.item = None;
                }
            });
        }
        Flow::Continue
    }

    /// `create_debug copycat <x> <y> <z>`: what a copycat holds, for a headless proof.
    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        event
            .commands
            .push(Command::op("create_debug copycat <pos:block_pos>", 2));
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        let c = event.command;
        if c.literals
            .iter()
            .map(String::as_str)
            .ne(["create_debug", "copycat"])
        {
            return Flow::Continue;
        }
        let Some(&CommandArgumentValue::BlockPos(pos)) = c.argument("pos") else {
            return Flow::Continue;
        };
        let dim = c.dimension.id(&*host).and_then(|d| level_index(&d));
        let line = match dim.and_then(|dim| with(dim, |d| d.copycats.get(&pos).cloned())) {
            Some(cat) => format!(
                "copycat at {} {} {}: {} ({})",
                pos[0],
                pos[1],
                pos[2],
                cat.material
                    .and_then(|m| host.block_state_string(m))
                    .unwrap_or_else(|| cc::BASE.to_owned()),
                cat.item
                    .map_or_else(|| "no item".to_owned(), |s| s.item().id(&*host))
            ),
            None => format!("no copycat at {} {} {}", pos[0], pos[1], pos[2]),
        };
        let _ = host.commands().feedback(Text::literal(line));
        Flow::Cancel
    }
}

/// Copycats placed (an empty one, unless the placement gave it a material), changed or broken
/// (`onRemove` with another block: its item drops); then each block entity whose copycat
/// changed shows its material. Nothing runs without a change.
fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let (changes, list) = COPYCATS.with_borrow_mut(|d| match d.get_mut(&dim) {
        Some(d) if !d.changes.is_empty() || !d.shown.is_empty() => {
            (std::mem::take(&mut d.changes), std::mem::take(&mut d.shown))
        }
        _ => (Vec::new(), Vec::new()),
    });
    if changes.is_empty() && list.is_empty() {
        return;
    }
    let block = |id: u32| states::state(id).and_then(|s| copycat_block(s.block()));
    let mut list = list;
    for (pos, old, new) in changes {
        let (was, is) = (block(old), block(new));
        if was.is_some() && was != is {
            sound::set_server(dim, pos, None);
            let item = with(dim, |d| d.copycats.remove(&pos)).and_then(|c| c.item);
            if let Some(item) = item {
                let [x, y, z] = center(pos);
                let _ = lv
                    .level
                    .spawn_item_with_motion(&mut *lv.host, x, y, z, item, [0.0; 3], 10);
                persist::mark_pos_unsaved(dim, pos);
            }
        }
        if is.is_some() {
            with(dim, |d| {
                d.copycats.entry(pos).or_default();
            });
        }
        list.push(pos);
    }
    list.sort_unstable();
    list.dedup();
    for pos in list {
        show(lv, pos);
    }
}

/// What the copycat at `pos` shows, put in its block entity for its clients.
fn show(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let material = with(lv.dim, |d| d.copycats.get(&pos).and_then(|c| c.material));
    // `getSoundType`: the material's.
    let sound = material.and_then(|m| lv.host.block_states().sound(m));
    sound::set_server(lv.dim, pos, sound);
    let (text, special) = match material {
        Some(m) => {
            let c = body::candidate(lv, m, pos);
            let special = match Special::of(&c) {
                Special::None => 0,
                Special::Bars => 1,
                Special::Trapdoor => 2,
            };
            (lv.host.block_state_string(m).unwrap_or_default(), special)
        }
        None => (String::new(), 0),
    };
    let data = Compound::from([
        (MATERIAL.into(), Tag::String(text.as_str().into())),
        (SPECIAL.into(), Tag::Byte(special)),
    ]);
    let _ = entity.merge_data(&mut *lv.host, data);
}

/// `CopycatBlockEntity.setMaterial`: a new block takes the orientation of a neighbour copycat
/// of the same state already showing that block; then clients are told.
fn set_material(lv: &mut ServerLevel<'_>, pos: [i32; 3], material: BlockState) {
    let dim = lv.dim;
    let own = lv.block_state(pos);
    let current = with(dim, |d| d.copycats.get(&pos).and_then(|c| c.material));
    let mut material = material;
    if current.map(BlockState::block) != Some(material.block()) {
        for side in Dir::ALL {
            let o = side.offset();
            let n = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            if lv.block_state(n) != own {
                continue;
            }
            let other = with(dim, |d| d.copycats.get(&n).and_then(|c| c.material));
            if let Some(other) = other
                && other.block() == material.block()
            {
                material = other;
                break;
            }
        }
    }
    with(dim, |d| {
        d.copycats.entry(pos).or_default().material = Some(material);
        d.shown.push(pos);
    });
}

/// A column's copycats (`CopycatBlockEntity.write`: `Material` and `Item`), as `create_copycat`:
/// `Pos` (three ints each), `Materials` (state text, empty for none) and `Items` (one compound
/// each, empty for none).
fn save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    let inside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] == chunk;
    let list: Vec<([i32; 3], Copycat)> = COPYCATS.with_borrow(|d| {
        d.get(&lv.dim).map_or_else(Vec::new, |d| {
            d.copycats
                .iter()
                .filter(|(p, _)| inside(p))
                .map(|(p, c)| (*p, c.clone()))
                .collect()
        })
    });
    if list.is_empty() {
        data.remove(&KEY.into());
        return;
    }
    let host = &*lv.host;
    let saved: Vec<cc::Saved> = list
        .iter()
        .map(|(p, c)| cc::Saved {
            pos: *p,
            material: c
                .material
                .and_then(|m| host.block_state_string(m))
                .unwrap_or_default(),
        })
        .collect();
    let (pos, materials) = cc::save(&saved);
    // `save` sorts by position: the items follow the same order.
    let mut list = list;
    list.sort_by_key(|x| x.0);
    let items = list
        .iter()
        .map(|(_, c)| {
            let nbt = c.item.as_ref().and_then(|s| s.to_nbt(host));
            Tag::Compound(nbt.unwrap_or_default())
        })
        .collect();
    data.insert(
        KEY.into(),
        Tag::Compound(Compound::from([
            ("Pos".into(), Tag::IntArray(pos)),
            (
                "Materials".into(),
                Tag::List(
                    materials
                        .iter()
                        .map(|m| Tag::String(m.as_str().into()))
                        .collect(),
                ),
            ),
            ("Items".into(), Tag::List(items)),
        ])),
    );
}

/// A column joined a level: its copycats back (`read`, a server's: a material stays only while
/// its saved item still gives it), each shown again.
fn load(lv: &mut ServerLevel<'_>, data: &Compound) {
    let Some(Tag::Compound(data)) = data.get(&KEY.into()) else {
        return;
    };
    let Some(Tag::IntArray(pos)) = data.get(&"Pos".into()) else {
        return;
    };
    let list = |key: &str| match data.get(&key.into()) {
        Some(Tag::List(l)) => l.to_vec(),
        _ => Vec::new(),
    };
    let materials: Vec<String> = list("Materials")
        .iter()
        .map(|t| match t {
            Tag::String(s) => s.to_string_lossy(),
            _ => String::new(),
        })
        .collect();
    let items = list("Items");
    for (i, saved) in cc::load(pos, &materials).into_iter().enumerate() {
        let item = match items.get(i) {
            Some(Tag::Compound(c)) if !c.is_empty() => ItemStack::from_nbt(&*lv.host, c),
            _ => None,
        };
        // No copycat there any more: nothing to keep.
        let Some(state) = lv.block_state(saved.pos) else {
            continue;
        };
        let Some(shape) = shape_of(&*lv.host, state) else {
            continue;
        };
        let material = lv.host.block_state_id(&saved.material);
        let keep = match (material, &item) {
            (Some(m), Some(it)) => {
                let given = body::accepted(lv, saved.pos, shape, it, None);
                let host = &*lv.host;
                let name = |s: BlockState| s.block().id(host);
                cc::keeps_material(
                    given.map(|g| name(g.0)).as_deref(),
                    given.is_some(),
                    &name(m),
                )
            }
            _ => false,
        };
        let c = if keep {
            Copycat { material, item }
        } else {
            Copycat::default()
        };
        with(lv.dim, |d| {
            d.copycats.insert(saved.pos, c);
            d.shown.push(saved.pos);
        });
    }
}
