//! Create's blocks that a vanilla template mostly runs: valve handles (a crank turned by an
//! angle, recoloured by dye), sliding doors (the iron door's template with Create's double doors,
//! `visible` and the sliding panel), toolboxes (recoloured by dye with their items kept, their
//! menu's screen laid out as compartments), postboxes (`open` while their menu is) and the train
//! and framed glass trapdoors. Rules live in `create_engine`.
//!
//! The toolboxes' block entity type, `create:toolbox`, is processing's, with the slots;
//! their menu's screen and its recolouring are here. A postbox's packages are the packages
//! port's (`port_stacks`).

use super::server::{ServerLevel, UPDATE_CLIENTS, update_all};
use super::states;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::door::{Door, Slide};
use create_engine::dye::{self, DyeColor};
use mod_api::{
    BlockEntityPart, BlockPart, BlockState, Compound, Hand, Host, HostTransaction,
    InteractionResult, ItemStack, MenuClosedEvent, PlayerRef, Registrar, RegistrationContext,
    Screen, ScreenEvent, ServerHost, SoundSource, UseItemOnEvent, UseWithoutItemEvent,
};

use super::processing::TOOLBOX_TYPE;

pub struct Blocks;

impl Subsystem for Blocks {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        for &block in Block::ALL.iter() {
            if dye::is_valve_handle(block)
                || dye::is_sliding_door(block)
                || dye::is_toolbox(block)
                || dye::is_postbox(block)
                || matches!(block, Block::TrainTrapdoor | Block::FramedGlassTrapdoor)
            {
                r.block_part(states::key(block), TemplateBlock);
            }
        }
        let toolbox = r.block_entity_type_key(TOOLBOX_TYPE);
        if let Err(e) = r.block_entity_part(&toolbox, ToolboxMenu) {
            crate::create_log!("[create] the toolbox screen's part was refused: {e:?}");
        }
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tick_doors(lv);
    }

    fn screen_render_post(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenRenderPostEvent<'_>,
    ) {
        toolbox_totals(host, event);
    }

    /// `PostboxBlockEntity.stopOpen`: the last menu closed on a postbox shuts it.
    fn menu_closed(&self, host: &mut dyn ServerHost, event: &mut MenuClosedEvent) {
        let Some(Some((dim, at))) = with_world(|w| w.postboxes.closed(event.uuid, event.window))
        else {
            return;
        };
        let Some(dimension) = super::world::level_id(dim) else {
            return;
        };
        if let Some(mut lv) = ServerLevel::new(host, &dimension) {
            set_postbox_open(&mut lv, at, false);
        }
    }
}

/// The Create block at `pos`, its state facts and its state.
fn block_at(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
) -> Option<(Block, create_engine::parts::StateInfo, BlockState)> {
    let state = lv.block_state(pos)?;
    let id = states::id(state)?;
    let (b, info) = with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten()?;
    Some((b, info, state))
}

/// `state` with `props` set.
fn with_props(host: &dyn Host, state: BlockState, props: &[(&str, &str)]) -> Option<BlockState> {
    let facts = mod_api::HostBlockStates::block_states(host);
    props
        .iter()
        .try_fold(state, |s, (k, v)| s.with(&facts, k, v).ok())
}

/// `BlockHelper.copyProperties(state, to.getDefaultState())`: block `to` with the state's
/// properties.
pub(crate) fn recoloured_state(
    host: &dyn Host,
    state: BlockState,
    to: Block,
) -> Option<BlockState> {
    copied_state(host, state, states::key(to))
}

/// `BlockHelper.copyProperties(state, to.defaultBlockState())` for any block `to`: the
/// properties `to` has take the state's values, the rest keep their defaults.
pub(crate) fn copied_state(
    host: &dyn Host,
    state: BlockState,
    to: mod_api::BlockKey,
) -> Option<BlockState> {
    let text = host.block_state_string(state)?;
    let (_, props) = states::split(&text);
    let base = host.block_state_default(to)?;
    let facts = mod_api::HostBlockStates::block_states(host);
    Some(
        props
            .iter()
            .fold(base, |s, (k, v)| s.with(&facts, k, v).unwrap_or(s)),
    )
}

fn sneaking(host: &dyn Host, player: PlayerRef) -> bool {
    player.entity().is_shift_key_down(host) == Some(true)
}

fn play(lv: &mut ServerLevel<'_>, sound: &str, pos: [i32; 3], volume: f32, pitch: f32) {
    let at = pos.map(|v| f64::from(v) + 0.5);
    lv.level
        .play_sound(&mut *lv.host, sound, at, SoundSource::Blocks, volume, pitch);
}

/// A pitch of `random.nextFloat() * 0.1 + 0.9`, from the level's random.
fn door_pitch(lv: &mut ServerLevel<'_>) -> f32 {
    use mod_api::RandomSource;
    lv.level
        .with_random(&mut *lv.host, |r| r.next_float())
        .map_or(0.9, |f| f * 0.1 + 0.9)
}

/// The valve handles, sliding doors, toolboxes, postboxes and Create's trapdoors.
struct TemplateBlock;

#[mod_api::horizon_part]
impl BlockPart for TemplateBlock {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let player = host.player(click.player)?;
        let dimension = click.dimension.key().id().to_owned();
        let mut lv = ServerLevel::new(host, &dimension)?;
        let (block, _, state) = block_at(&lv, click.pos)?;
        let back = sneaking(&*lv.host, player);
        let item = click.stack.map(|s| s.item().id(&*lv.host));
        if dye::is_valve_handle(block) {
            use_valve_handle(
                &mut lv,
                player,
                click.pos,
                block,
                state,
                item.as_deref(),
                back,
            )
        } else if dye::is_toolbox(block) {
            use_toolbox(&mut lv, click.pos, block, state, item.as_deref(), back)
        } else {
            None
        }
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let player = host.player(click.player)?;
        let dimension = click.dimension.key().id().to_owned();
        let mut lv = ServerLevel::new(host, &dimension)?;
        let (block, info, state) = block_at(&lv, click.pos)?;
        let back = sneaking(&*lv.host, player);
        if dye::is_sliding_door(block) {
            use_door(&mut lv, click.pos, block, &info)
        } else if dye::is_postbox(block) {
            use_postbox(&mut lv, player, click.pos, block, click.hand, back)
        } else if matches!(block, Block::TrainTrapdoor | Block::FramedGlassTrapdoor) {
            use_trapdoor(&mut lv, click.pos, state, info.open)
        } else {
            None
        }
    }
}

/// `ValveHandleBlock.onBlockActivated` → `clicked`: not for a player who may not build, nor a
/// sneaking wrench; a dye of another colour recolours it (`DYED_VALVE_HANDLES`, its properties
/// kept), else it turns (`activate`). The click is taken either way.
pub(crate) fn use_valve_handle(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    block: Block,
    state: BlockState,
    item: Option<&str>,
    back: bool,
) -> Option<InteractionResult> {
    if player.may_build(&*lv.host) != Some(true) {
        return None;
    }
    if back && item == Some("create:wrench") {
        return None;
    }
    if let Some(to) = item
        .and_then(DyeColor::of_item)
        .and_then(|c| dye::recoloured(block, c))
    {
        if let Some(next) = recoloured_state(&*lv.host, state, to) {
            // `ValveHandleBlock.onRemove`: another valve handle keeps the block entity, and so
            // its angle.
            let kept = lv
                .host
                .block_entity(lv.level, pos)
                .and_then(|e| e.data(&*lv.host))
                .and_then(|d| {
                    let key = super::sources::handles::SCROLL_VALUE;
                    Some(Compound::from([(key.into(), d.get(&key.into())?.clone())]))
                });
            lv.set_state(pos, next, update_all());
            if let (Some(kept), Some(entity)) = (kept, lv.host.block_entity(lv.level, pos)) {
                let _ = entity.merge_data(&mut *lv.host, kept);
            }
        }
        // ValveHandleBlock.clicked keeps the entity; publish its cleared kinetic
        // fields and the shafts' live state before a same-tick save.
        super::persist::publish(lv);
        return Some(InteractionResult::Success);
    }
    let dim = lv.dim;
    let entity = lv.host.block_entity(lv.level, pos);
    let value = entity
        .and_then(|e| e.data(&*lv.host))
        .map_or(create_engine::valve::DEFAULT_ANGLE, |d| {
            super::sources::handles::valve_angle(&d)
        });
    if with_world(|w| w.use_valve(dim, pos, back, value)) == Some(true)
        && let Some(entity) = entity
    {
        let used = super::sources::handles::valve_use(lv.game_time(), value, back);
        let _ = entity.merge_data(&mut *lv.host, used);
    }
    super::persist::publish(lv);
    Some(InteractionResult::Success)
}

/// `ToolboxBlock.useItemOn`: sneaking passes (empty-handed, nothing opens); a dye of another
/// colour recolours it, its items kept (`onRemove` keeps the block entity for another
/// toolbox); else its menu opens (the template's use).
fn use_toolbox(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    state: BlockState,
    item: Option<&str>,
    back: bool,
) -> Option<InteractionResult> {
    if back {
        return item.is_none().then_some(InteractionResult::Success);
    }
    let to = item
        .and_then(DyeColor::of_item)
        .and_then(|c| dye::recoloured(block, c))?;
    let Some(next) = recoloured_state(&*lv.host, state, to) else {
        return Some(InteractionResult::Success);
    };
    let slots = create_engine::toolbox::STORED as u32;
    let items: Vec<(u32, ItemStack)> = lv
        .host
        .block_entity(lv.level, pos)
        .map(|be| {
            (0..slots)
                .filter_map(|i| Some((i, be.item_slot(&*lv.host, i).ok()??)))
                .collect()
        })
        .unwrap_or_default();
    lv.set_state(pos, next, update_all());
    if !items.is_empty()
        && let Some(be) = lv.host.block_entity(lv.level, pos)
    {
        let _ = lv.host.transaction(|host, tx| {
            for (i, stack) in items {
                let empty = be.item_slot(&*host, i).ok().flatten().is_none();
                if empty {
                    be.set_item_slot(host, tx, i, Some(stack)).map_err(|_| ())?;
                }
            }
            Ok::<(), ()>(())
        });
    }
    Some(InteractionResult::Success)
}

/// `PostboxBlock.useWithoutItem` → `PackagePortBlockEntity.use`: the main hand's click, not
/// sneaking, opens its screen (packages' `port_screen`); it shows open until the screen closes
/// (`startOpen`, `stopOpen`).
fn use_postbox(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    block: Block,
    hand: Hand,
    back: bool,
) -> Option<InteractionResult> {
    if back || hand != Hand::Main {
        return None;
    }
    let _ = block;
    if super::packages::port_clipboard(lv, player, pos) {
        return Some(InteractionResult::Success);
    }
    super::packages::port_screen::open(lv, player, pos, false);
    set_postbox_open(lv, pos, true);
    Some(InteractionResult::Success)
}

/// The port screen of the postbox at `pos` closed: it shuts.
pub(crate) fn postbox_closed(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    set_postbox_open(lv, pos, false);
}

/// `PostboxBlockEntity.onOpenChange`: `open` set (`setBlockAndUpdate`) with a barrel's sound, if
/// the block is still a postbox.
fn set_postbox_open(lv: &mut ServerLevel<'_>, pos: [i32; 3], open: bool) {
    if !block_at(lv, pos).is_some_and(|(b, _, _)| dye::is_postbox(b)) {
        return;
    }
    let value = if open { "true" } else { "false" };
    lv.set_property(pos, "open", value, update_all());
    let sound = if open {
        "minecraft:block.barrel.open"
    } else {
        "minecraft:block.barrel.close"
    };
    play(lv, sound, pos, 1.0, 1.0);
}

/// `TrainTrapdoorBlock.useWithoutItem`: a click opens or closes it whatever its power
/// (`state.cycle(OPEN)`, `UPDATE_CLIENTS`), with its `BlockSetType`'s iron trapdoor sounds.
fn use_trapdoor(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    state: BlockState,
    open: bool,
) -> Option<InteractionResult> {
    let open = !open;
    let next = with_props(
        &*lv.host,
        state,
        &[("open", if open { "true" } else { "false" })],
    )?;
    lv.set_state(pos, next, UPDATE_CLIENTS);
    let sound = if open {
        "minecraft:block.iron_trapdoor.open"
    } else {
        "minecraft:block.iron_trapdoor.close"
    };
    let pitch = door_pitch(lv);
    play(lv, sound, pos, 1.0, pitch);
    Some(InteractionResult::Success)
}

/// The sliding door at `pos`: its block, door and state.
fn door_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Block, Door, BlockState)> {
    let (b, info, state) = block_at(lv, pos)?;
    dye::is_sliding_door(b)
        .then(|| Door::of(&info))
        .flatten()
        .map(|d| (b, d, state))
}

/// Both halves of the door at `pos` set `open` (and hidden while open), as `setBlock` of one
/// half and the other's `updateShape` (which copies it, `visible` too) leave them.
fn set_door_open(lv: &mut ServerLevel<'_>, pos: [i32; 3], door: &Door, open: bool) {
    for at in [pos, door.other_half(pos)] {
        let Some((_, _, state)) = door_at(lv, at) else {
            continue;
        };
        let props: &[(&str, &str)] = if open {
            &[("open", "true"), ("visible", "false")]
        } else {
            &[("open", "false")]
        };
        if let Some(next) = with_props(&*lv.host, state, props).filter(|s| *s != state) {
            // UPDATE_CLIENTS | UPDATE_IMMEDIATE
            lv.set_state(at, next, UPDATE_CLIENTS);
        }
    }
}

/// `SlidingDoorBlock.useWithoutItem`: the click opens or closes the door; a double door's other
/// door follows (`tick_doors`), and an iron door's opening sound plays at the last door it
/// reached.
fn use_door(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    info: &create_engine::parts::StateInfo,
) -> Option<InteractionResult> {
    let door = Door::of(info)?;
    let open = !door.open;
    set_door_open(lv, pos, &door, open);
    if !open {
        return Some(InteractionResult::Success);
    }
    let partner = door.partner(pos);
    // `isDoubleDoor(state.cycle(OPEN), ...)`: the other door not yet as open as this one now.
    let double = door_at(lv, partner)
        .is_some_and(|(b, other, _)| b == block && Door { open, ..door }.is_double(&other));
    let at = if double { partner } else { pos };
    let pitch = door_pitch(lv);
    play(lv, "minecraft:block.iron_door.open", at, 1.0, pitch);
    Some(InteractionResult::Success)
}

/// Sliding doors after the level's tick: a door that opened is hidden (`visible` false, as
/// Create sets it with `open`) and takes the other door of a double door with it, and its panel
/// slides; once a closed door's panel has slid shut it shows again, with an iron door's
/// closing sound. Only doors that changed or still slide cost anything.
fn tick_doors(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let idle = |w: &mut create_engine::world::CreateWorld| {
        w.dims[dim].door_changes.is_empty() && w.dims[dim].doors.is_empty()
    };
    if with_world(idle) != Some(false) {
        return;
    }
    // A double door's other door changes in this pass too.
    for _ in 0..4 {
        let changes =
            with_world(|w| std::mem::take(&mut w.dims[dim].door_changes)).unwrap_or_default();
        if changes.is_empty() {
            break;
        }
        for (pos, was_open) in changes {
            let Some((block, door, state)) = door_at(lv, pos) else {
                continue;
            };
            if door.open
                && door.visible
                && let Some(next) = with_props(&*lv.host, state, &[("visible", "false")])
            {
                lv.set_state(pos, next, UPDATE_CLIENTS);
            }
            let partner = door.partner(pos);
            if let Some((b, other, _)) = door_at(lv, partner)
                && b == block
                && door.is_double(&other)
            {
                set_door_open(lv, partner, &other, door.open);
            }
            if !door.upper {
                with_world(|w| {
                    w.dims[dim]
                        .doors
                        .entry(pos)
                        .or_insert_with(|| Slide::at_rest(was_open));
                });
            }
        }
    }
    let moving: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].doors.keys().copied().collect()).unwrap_or_default();
    for pos in moving {
        let Some((_, door, state)) = door_at(lv, pos).filter(|(_, d, _)| !d.upper) else {
            with_world(|w| w.dims[dim].doors.remove(&pos));
            continue;
        };
        let Some((show, done)) = with_world(|w| {
            let s = w.dims[dim].doors.get_mut(&pos)?;
            let show = s.tick(door.open, door.visible);
            Some((show, s.at_target()))
        })
        .flatten() else {
            continue;
        };
        if show {
            // `showBlockModel`: UPDATE_ALL, the upper half following through `updateShape`.
            for (at, s) in [(pos, Some(state)), (door.other_half(pos), None)] {
                let s = s.or_else(|| door_at(lv, at).map(|(_, _, s)| s));
                if let Some(next) = s.and_then(|s| with_props(&*lv.host, s, &[("visible", "true")]))
                {
                    lv.set_state(at, next, update_all());
                }
            }
            play(lv, "minecraft:block.iron_door.close", pos, 0.5, 1.0);
        }
        if done {
            with_world(|w| w.dims[dim].doors.remove(&pos));
        }
    }
}

/// The toolbox's menu screen.
struct ToolboxMenu;

#[mod_api::horizon_part]
impl BlockPart for ToolboxMenu {}

#[mod_api::horizon_part]
impl BlockEntityPart for ToolboxMenu {
    /// A toolbox's screen (`ToolboxScreen`) shows each compartment by its first stack round the
    /// toolbox's picture; the other stacks and the container's four slots past Create's 32 are
    /// hidden (the server's menu leaves them out too).
    fn screen(&self, _host: &dyn Host, event: &mut ScreenEvent<'_>) -> Option<Screen> {
        use create_engine::toolbox;
        let block = event
            .block
            .strip_prefix("create:")
            .and_then(Block::from_name)?;
        if !dye::is_toolbox(block) {
            return None;
        }
        Some(Screen {
            width: toolbox::SCREEN_WIDTH,
            height: toolbox::SCREEN_HEIGHT,
            slots: (0..toolbox::SLOTS).map(toolbox::slot_position).collect(),
            inventory_y: toolbox::INVENTORY_Y,
            inventory_x: toolbox::INVENTORY_X,
            own_labels: true,
            own_background: true,
            recipe_book: None,
            recipe_book_toggle: None,
        })
    }

    /// `ToolboxScreen.renderBg`: the toolbox's background with its name, the player's inventory
    /// under it, the deposit button and the confirm button.
    fn screen_draw(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenDrawEvent<'_>,
    ) {
        use create_engine::toolbox::{BACKGROUND, SCREEN_HEIGHT};
        let Some(block) = toolbox_block(event.block) else {
            return;
        };
        let [ox, oy] = event.origin;
        let draw = &mut *event.draw;
        let bx = ox + 30.0;
        draw.sprite(
            "create:gui/toolbox",
            [0.0, 0.0, BACKGROUND[0], BACKGROUND[1]],
            [bx, oy, BACKGROUND[0], BACKGROUND[1]],
            None,
        );
        let title = host
            .translate(&format!("block.create.{}", block.name()))
            .unwrap_or_default();
        draw.text(&title, [bx + 15.0, oy + 4.0], 0xFF59_2424, false);
        let iy = oy + SCREEN_HEIGHT - 108.0;
        draw.sprite(
            "create:gui/player_inventory",
            [0.0, 0.0, 176.0, 108.0],
            [ox, iy, 176.0, 108.0],
            None,
        );
        draw.text(
            &host.translate("container.inventory").unwrap_or_default(),
            [ox + 8.0, iy + 6.0],
            0xFF40_4040,
            false,
        );
        // `renderToolbox`: the toolbox, 50 px a block, right of the background's foot.
        if let Some(state) = host.block_state_id(event.block) {
            let _ = draw.draw_scene(
                [
                    bx + BACKGROUND[0],
                    oy + 116.0,
                    bx + BACKGROUND[0] + 72.0,
                    oy + 184.0,
                ],
                &mod_api::Scene::new(vec![mod_api::SceneBlock::new([0, 0, 0], state, None)]),
                mod_api::SceneView::new([0.5, 0.5, 0.5], -202.0, 22.0, 50.0),
            );
        }
        for (at, icon) in [(TOOLBOX_DEPOSIT, I_TOOLBOX), (toolbox_confirm(), I_CONFIRM)] {
            let (x, y) = (ox + at[0], oy + at[1]);
            draw.sprite(
                "create:gui/widgets",
                [0.0, 0.0, 18.0, 18.0],
                [x, y, 18.0, 18.0],
                None,
            );
            draw.sprite(
                "create:gui/icons",
                [icon[0], icon[1], 16.0, 16.0],
                [x + 1.0, y + 1.0, 16.0, 16.0],
                None,
            );
        }
    }

    /// The deposit button (`ToolboxDisposeAllPacket`) and the confirm button (closes).
    fn screen_click(
        &self,
        _host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenClickEvent<'_>,
    ) -> Option<i32> {
        toolbox_block(event.block)?;
        if event.right {
            return None;
        }
        if on_button(TOOLBOX_DEPOSIT, event.at) {
            Some(TOOLBOX_DEPOSIT_BUTTON)
        } else if on_button(toolbox_confirm(), event.at) {
            Some(TOOLBOX_CONFIRM_BUTTON)
        } else {
            None
        }
    }

    /// The deposit button's `create.toolbox.depositBox`.
    fn screen_tooltip(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenTooltipEvent<'_>,
    ) {
        if toolbox_block(event.block).is_some() && on_button(TOOLBOX_DEPOSIT, event.at) {
            event.lines.push(
                host.translate("create.toolbox.depositBox")
                    .unwrap_or_default(),
            );
        }
    }

    fn menu_button(&self, host: &mut dyn ServerHost, event: &mut mod_api::MenuButtonEvent<'_>) {
        if event.entity.id() != TOOLBOX_TYPE {
            return;
        }
        match event.context.id {
            TOOLBOX_CONFIRM_BUTTON => host.close_menu(event.context.player),
            TOOLBOX_DEPOSIT_BUTTON => deposit_all(host, event),
            _ => {}
        }
    }
}

/// `ToolboxScreen.renderBg`: each compartment shows its first stack with the compartment's total
/// (`totalCountInCompartment`) for its count. The screen's slots draw their own stack's count, so
/// over a compartment holding more than its first stack the slot is drawn again: the
/// background under it, the stack with the total, and the hover highlight. Not while a stack is
/// carried over it.
fn toolbox_totals(host: &dyn mod_api::ClientHost, event: &mut mod_api::ScreenRenderPostEvent<'_>) {
    use create_engine::toolbox::{STACKS_PER_COMPARTMENT, STORED};
    let Some(pos) = host.screen_pos() else {
        return;
    };
    let toolbox = host
        .level(host.dimension())
        .and_then(|l| l.block_state(host, pos))
        .map(states::id_or_none)
        .and_then(|id| super::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten())
        .is_some_and(dye::is_toolbox);
    if !toolbox {
        return;
    }
    let Some(menu) = host.local_player().and_then(|p| p.container_menu(host)) else {
        return;
    };
    let Some(boxes) = menu.slot_boxes(host) else {
        return;
    };
    let carried = menu.carried(host).flatten().is_some();
    let mouse = event.screen.mouse;
    let [ox, oy] = [event.screen.rect[0], event.screen.rect[1]];
    for first in (0..STORED).step_by(STACKS_PER_COMPARTMENT) {
        let Some(Some([x, y, w, h])) = boxes.get(first).copied() else {
            continue;
        };
        let Some(stack) = menu.slot(host, first).flatten() else {
            continue;
        };
        let total: u32 = (first..first + STACKS_PER_COMPARTMENT)
            .filter_map(|i| menu.slot(host, i).flatten())
            .map(|s| s.count())
            .sum();
        if total == stack.count() {
            continue;
        }
        let hovered = (x..x + w).contains(&mouse[0]) && (y..y + h).contains(&mouse[1]);
        if hovered && carried {
            continue;
        }
        let draw = &mut *event.draw;
        // The background under the slot (`AllGuiTextures.TOOLBOX`, 30 px in).
        draw.sprite(
            "create:gui/toolbox",
            [x - ox - 30.0, y - oy, 16.0, 16.0],
            [x, y, 16.0, 16.0],
            None,
        );
        draw.gui_draw_item_with(&stack, [x, y], Some(&total.to_string()));
        if hovered {
            draw.fill([x, y, 16.0, 16.0], 0x80FF_FFFF);
        }
    }
}

/// `AllIcons.I_TOOLBOX` and `I_CONFIRM` in `icons.png`.
const I_TOOLBOX: [f32; 2] = [176.0, 48.0];
const I_CONFIRM: [f32; 2] = [0.0, 16.0];
/// The deposit button, `(30 + 81, 69)` in the window.
const TOOLBOX_DEPOSIT: [f32; 2] = [111.0, 69.0];
const TOOLBOX_DEPOSIT_BUTTON: i32 = 0;
const TOOLBOX_CONFIRM_BUTTON: i32 = 1;

/// The toolbox named by a screen's `block`, if it is one.
fn toolbox_block(block: &str) -> Option<Block> {
    block
        .strip_prefix("create:")
        .and_then(Block::from_name)
        .filter(|b| dye::is_toolbox(*b))
}

/// Whether `at` is on the 18 px button at `button`.
fn on_button(button: [f32; 2], at: [f32; 2]) -> bool {
    (button[0]..button[0] + 18.0).contains(&at[0]) && (button[1]..button[1] + 18.0).contains(&at[1])
}

/// The confirm button: `30 + BG.width - 33`, `BG.height - 24`.
fn toolbox_confirm() -> [f32; 2] {
    let bg = create_engine::toolbox::BACKGROUND;
    [30.0 + bg[0] - 33.0, bg[1] - 24.0]
}

/// `ToolboxDisposeAllPacket`: each of the player's 36 stacks goes into the compartment that
/// already holds its kind (`inLimitedMode`), onto its stacks and then its empty stacks
/// (`insertItemStacked`).
fn deposit_all(host: &mut dyn ServerHost, event: &mut mod_api::MenuButtonEvent<'_>) {
    use create_engine::toolbox::{STACKS_PER_COMPARTMENT, STORED};
    let Some(player) = host.player(event.context.player) else {
        return;
    };
    let Some(slots) = player.inventory_slots(&*host) else {
        return;
    };
    let items = &mut event.entity.items;
    for (slot, stack) in slots.into_iter().filter(|(s, _)| *s < 36) {
        let mut left = stack.count();
        let max = host.max_stack_size(stack.item()).unwrap_or(64);
        for c in 0..STORED / STACKS_PER_COMPARTMENT {
            let range = c * STACKS_PER_COMPARTMENT..(c + 1) * STACKS_PER_COMPARTMENT;
            // The compartment's kind: its first stack that holds anything.
            let kind = range.clone().find_map(|i| items.get(i).cloned().flatten());
            if !kind.is_some_and(|k| k.item() == stack.item() && k.same_components(&stack)) {
                continue;
            }
            for i in range.clone() {
                if let Some(Some(held)) = items.get_mut(i) {
                    let room = max.saturating_sub(held.count()).min(left);
                    if room > 0 && held.set_count(held.count() + room).is_ok() {
                        left -= room;
                    }
                }
            }
            for i in range {
                if left == 0 {
                    break;
                }
                if let Some(empty @ None) = items.get_mut(i) {
                    let n = left.min(max);
                    let mut put = stack.clone();
                    if put.set_count(n).is_ok() {
                        left -= n;
                        *empty = Some(put);
                    }
                }
            }
            break;
        }
        if left != stack.count() {
            let mut rest = stack;
            let rest = (left > 0 && rest.set_count(left).is_ok()).then_some(rest);
            let _ = player.set_inventory_slot(host, slot, rest);
        }
    }
}
