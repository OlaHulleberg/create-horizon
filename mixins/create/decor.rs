// Create's decorative and simple blocks in the game: girders and their encased shafts, brackets on
// shafts, cogs and pipes, the large water wheel's frame, placards, the desk bell and the peculiar
// bell's haunting, the creative crate, the item hatch, clipboards, table cloths and cardboard, and
// the vertical gearbox and empty blaze burner items. A sibling of the `create` module
// (`runtime.rs`); the rules are `create_engine::decor`'s. What blocks hold (brackets, placards'
// items, crates' supplies, hatches' filters, clipboards) is typed, per dimension, saved with its
// column as `create_decor`; only placards, bells and hatches with a timer running are looked at
// each tick.
use super::Game;
use super::create::*;
use super::create_processing::CLICKED;
use create_engine::Block;
use create_engine::decor::{self, Bracket, BracketKind, Side};
use create_engine::kinetics::{Axis, Dir};
use create_engine::parts::StateInfo;
use create_engine::states;
use create_engine::world::dimension_index;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, Tag};
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::HashMap;

/// A timer running on a block: due at a game time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Timer {
    /// `PlacardBlockEntity.poweredTicks` ran out.
    Placard,
    /// `DeskBellBlockEntity.blockStateTimer` ran out.
    Bell,
    /// `ItemHatchBlock.tick`: it closes.
    Hatch,
}

/// One dimension's decorative state.
#[derive(Default)]
struct Dim {
    brackets: HashMap<[i32; 3], Bracket>,
    placards: HashMap<[i32; 3], ItemStack>,
    supplies: HashMap<[i32; 3], String>,
    hatch_filters: HashMap<[i32; 3], String>,
    clipboards: HashMap<[i32; 3], ItemStack>,
    timers: Vec<([i32; 3], i64, Timer)>,
    /// Girders and encased shafts whose neighbour changed (`updateShape`), peculiar bells to
    /// look under.
    girder_checks: Vec<[i32; 3]>,
    bell_checks: Vec<[i32; 3]>,
    /// Blocks whose bracket or placard item clients must see again.
    shown: Vec<[i32; 3]>,
}

/// A block of interest changed: dimension, position, old and new state.
type Change = (usize, [i32; 3], u32, u32);

thread_local! {
    static DECOR: RefCell<Option<Box<[Dim; 3]>>> = const { RefCell::new(None) };
    static CHANGES: RefCell<Vec<Change>> = const { RefCell::new(Vec::new()) };
}

fn with<R>(dim: usize, f: impl FnOnce(&mut Dim) -> R) -> R {
    DECOR.with_borrow_mut(|d| f(&mut d.get_or_insert_with(Default::default)[dim]))
}

const KEY: &str = "create_decor";

/// The world closed: everything is forgotten.
pub(super) fn forget() {
    DECOR.with_borrow_mut(|d| *d = None);
    CHANGES.with_borrow_mut(Vec::clear);
}

fn block_of(state: u32) -> Option<(Block, StateInfo)> {
    with_world(|w| w.table.get(state).map(|(b, i)| (b, *i))).flatten()
}

/// Blocks a bracket goes on (`BracketedBlockEntityBehaviour`: shafts and cogs,
/// `AbstractSimpleShaftBlock`; fluid pipes).
fn bracketed(b: Block) -> bool {
    matches!(
        b,
        Block::Shaft | Block::Cogwheel | Block::LargeCogwheel | Block::FluidPipe
    )
}

fn ours(b: Block) -> bool {
    bracketed(b)
        || matches!(
            b,
            Block::MetalGirder
                | Block::MetalGirderEncasedShaft
                | Block::Placard
                | Block::Clipboard
                | Block::CreativeCrate
                | Block::ItemHatch
                | Block::PeculiarBell
        )
}

/// A Create block changed (after the hot hook's range test): ours are looked at after the
/// level's tick.
pub(super) fn block_changed(dim: usize, pos: [i32; 3], old: u32, new: u32) {
    let (o, n) = (block_of(old).map(|x| x.0), block_of(new).map(|x| x.0));
    // Shafts, cogs and pipes are only looked at when they hold a bracket.
    let mine = |b: Option<Block>| b.is_some_and(|b| ours(b) && !bracketed(b));
    let bracket = o.is_some_and(bracketed)
        && DECOR.with_borrow(|d| {
            d.as_ref()
                .is_some_and(|d| d[dim].brackets.contains_key(&pos))
        });
    if mine(o) || mine(n) || bracket {
        CHANGES.with_borrow_mut(|c| c.push((dim, pos, old, new)));
    }
}

/// A neighbour of a Create block changed: girders and encased shafts shape themselves again
/// (`updateShape`), a peculiar bell looks at what is under it.
pub(super) fn neighbor_changed(dim: usize, pos: [i32; 3], state: u32) {
    match block_of(state).map(|(b, _)| b) {
        Some(Block::MetalGirder | Block::MetalGirderEncasedShaft) => {
            with(dim, |d| d.girder_checks.push(pos))
        }
        Some(Block::PeculiarBell) => with(dim, |d| d.bell_checks.push(pos)),
        _ => {}
    }
}

fn add(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

fn center(p: [i32; 3]) -> [f64; 3] {
    p.map(|v| f64::from(v) + 0.5)
}

fn dir_named(v: &str) -> Option<Dir> {
    Dir::ALL.into_iter().find(|d| dir_name(*d) == v)
}

fn sneaking(game: &Game<'_>) -> bool {
    game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft)
}

/// The main hand's stack.
fn held_stack(game: &Game<'_>) -> Option<ItemStack> {
    game.inventory.get(game.selected).cloned().flatten()
}

/// One of the main hand's item used up (not in creative).
fn use_one(game: &mut Game<'_>) {
    if game.creative {
        return;
    }
    if let Some(slot) = game.inventory.get_mut(game.selected) {
        match slot {
            Some(s) if s.count > 1 => s.count -= 1,
            _ => *slot = None,
        }
    }
}

fn set_state(game: &mut Game<'_>, pos: [i32; 3], id: BlockStateId) {
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
}

/// The state at `pos` with `props` changed; None when it has no such properties.
fn with_props(game: &Game<'_>, pos: [i32; 3], props: &[(&str, &str)]) -> Option<BlockStateId> {
    let registry = game.pack.assets.registry;
    let mut s = registry
        .state(game.world.block(pos[0], pos[1], pos[2]))?
        .clone();
    for (k, v) in props {
        s.properties.insert((*k).to_owned(), (*v).to_owned());
    }
    registry.id(&s)
}

fn flag(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

fn mark_unsaved(game: &mut Game<'_>, pos: [i32; 3]) {
    let dimension = game.dim.id().to_owned();
    game.native_mark_chunk_unsaved(&dimension, [pos[0] >> 4, pos[2] >> 4]);
}

fn drop_at(game: &mut Game<'_>, pos: [i32; 3], stack: ItemStack) {
    super::create_contraption_body::drop_stack(game, center(pos), [0.0; 3], stack);
}

// ---------------------------------------------------------------------------------------------
// Girders

/// What a girder at `pos` reads of the block on side `d` (`GirderBlock.updateState`,
/// `updateVerticalProperty`, `isConnected`).
fn side(game: &Game<'_>, pos: [i32; 3], d: Dir) -> Side {
    let p = add(pos, d);
    let id = game.world.block(p[0], p[1], p[2]);
    let b = game.res.behaviours();
    let Some(s) = game.pack.assets.registry.state(id) else {
        return Side::default();
    };
    let prop = |k: &str| s.properties.get(k).map(String::as_str);
    let empty = match b.collision_shape(id) {
        Some(shape) => shape.is_empty(),
        None => b.facts(id).flags & horizon_world::level::data::state_flags::COLLISION_EMPTY != 0,
    };
    let name = s.name.as_str();
    let face_dir = |facing: &str| {
        dir_named(facing).map(|f| decor::connected_direction(prop("face").unwrap_or(""), f))
    };
    let bracket = dimension_index(game.dim.id())
        .and_then(|dim| with(dim, |x| x.brackets.get(&p).copied()))
        .is_some_and(|br| br.facing == d);
    let toward =
        bracket || (name == "create:placard" && prop("facing").and_then(face_dir) == Some(d));
    let holds = d.axis() == Axis::Y
        && (prop("up") == Some("true")
            || name == "create:display_board"
            || (name.ends_with("lantern")
                && prop("hanging").is_some_and(|h| (h == "true") == (d == Dir::Down)))
            || (name.ends_with("chain") && prop("axis") == Some("y"))
            || (prop("face") == Some("ceiling") && d == Dir::Down)
            || (prop("face") == Some("floor") && d == Dir::Up));
    Side {
        air: b.is_air(id),
        girder: (name == "create:metal_girder")
            .then(|| (prop("x") == Some("true"), prop("z") == Some("true"))),
        encased: (name == "create:metal_girder_encased_shaft").then(|| {
            if prop("axis") == Some("x") {
                Axis::X
            } else {
                Axis::Z
            }
        }),
        toward,
        holds,
        full_face: game.res.facts().get(id).sturdy_full & (1 << d.opposite() as u8) != 0,
        empty,
        chute_down: (name == "create:chute" && prop("facing") == Some("down"))
            || name == "create:smart_chute",
    }
}

/// A girdable track (`create:track`) above beside `pos` across `d`'s axis, running along it
/// (`TrackShape.XO` / `ZO`).
fn track_beside(game: &Game<'_>, pos: [i32; 3], d: Dir) -> bool {
    let axis = d.axis();
    if axis == Axis::Y {
        return false;
    }
    let across = if axis == Axis::X { Axis::Z } else { Axis::X };
    let want = if axis == Axis::X { "xo" } else { "zo" };
    [true, false].into_iter().any(|positive| {
        let p = add(
            [pos[0], pos[1] + 1, pos[2]],
            Dir::from_axis(across, positive),
        );
        game.pack
            .assets
            .registry
            .state(game.world.block(p[0], p[1], p[2]))
            .is_some_and(|s| {
                s.name == "create:track"
                    && s.properties.get("shape").map(String::as_str) == Some(want)
            })
    })
}

fn girder_of(info: &StateInfo, props: &dyn Fn(&str) -> bool) -> decor::Girder {
    decor::Girder {
        axis: info.axis.unwrap_or(Axis::Y),
        x: props("x"),
        z: props("z"),
        top: props("top"),
        bottom: props("bottom"),
    }
}

fn axis_name(a: Axis) -> &'static str {
    ["x", "y", "z"][axis_index(a) as usize]
}

fn set_girder(game: &mut Game<'_>, pos: [i32; 3], g: decor::Girder) {
    let axis = axis_name(g.axis);
    if let Some(id) = with_props(
        game,
        pos,
        &[
            ("axis", axis),
            ("x", flag(g.x)),
            ("z", flag(g.z)),
            ("top", flag(g.top)),
            ("bottom", flag(g.bottom)),
        ],
    ) && id != game.world.block(pos[0], pos[1], pos[2])
    {
        set_state(game, pos, id);
    }
}

/// A girder or encased shaft shapes itself again after its neighbours changed: every side, as
/// `updateShape` for each (the hook doesn't say which neighbour it was).
fn reshape_girder(game: &mut Game<'_>, pos: [i32; 3]) {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let Some((block, info)) = block_of(id.0) else {
        return;
    };
    let registry = game.pack.assets.registry;
    let Some(s) = registry.state(id).cloned() else {
        return;
    };
    let props = |k: &str| s.properties.get(k).is_some_and(|v| v == "true");
    let support_empty = |d: Dir| {
        let x = side(game, pos, d);
        x.air || x.empty
    };
    match block {
        Block::MetalGirder => {
            let mut g = girder_of(&info, &props);
            for d in Dir::ALL {
                g = decor::update_shape(
                    g,
                    d,
                    |d| side(game, pos, d),
                    support_empty,
                    |d| track_beside(game, pos, d),
                );
            }
            set_girder(game, pos, g);
        }
        Block::MetalGirderEncasedShaft => {
            let mut tb = (props("top"), props("bottom"));
            for d in [Dir::Up, Dir::Down] {
                tb = decor::encased_vertical(tb, d, &side(game, pos, d), support_empty(d));
            }
            if tb != (props("top"), props("bottom"))
                && let Some(id) =
                    with_props(game, pos, &[("top", flag(tb.0)), ("bottom", flag(tb.1))])
            {
                set_state(game, pos, id);
            }
        }
        _ => {}
    }
}

/// The girders and encased shafts beside `pos` shape themselves again (a bracket came or went:
/// `updateNeighbourShapes`).
fn girders_around(dim: usize, pos: [i32; 3]) {
    with(dim, |d| {
        for side in Dir::ALL {
            d.girder_checks.push(add(pos, side));
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Brackets

/// `FluidPropagator.getStraightPipeAxis` of a fluid pipe's state: the axis of its only two open
/// sides when they face each other.
fn straight_pipe_axis(open: impl Fn(Dir) -> bool) -> Option<Axis> {
    let mut found = None;
    let mut connections = 0;
    for axis in [Axis::X, Axis::Y, Axis::Z] {
        let (a, b) = (
            open(Dir::from_axis(axis, false)),
            open(Dir::from_axis(axis, true)),
        );
        connections += usize::from(a) + usize::from(b);
        if a && b {
            if found.is_some() {
                return None;
            }
            found = Some(axis);
        }
    }
    (connections == 2).then_some(found).flatten()
}

/// `BracketBlockItem.useOn` on a shaft, cog or pipe: the bracket for the clicked face (else the
/// face toward the player) replaces any it had, which goes back to the player.
fn use_bracket(game: &mut Game<'_>, wooden: bool, pos: [i32; 3], face: [i32; 3]) -> bool {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let Some((block, info)) = block_of(id.0).filter(|(b, _)| bracketed(*b)) else {
        // `behaviour == null`: FAIL, nothing is placed.
        return true;
    };
    let Some(dim) = dimension_index(game.dim.id()) else {
        return true;
    };
    let (kind, target) = match block {
        Block::Shaft => (BracketKind::Shaft, info.axis),
        Block::FluidPipe => {
            let s = game.pack.assets.registry.state(id).cloned();
            let open = |d: Dir| {
                s.as_ref()
                    .and_then(|s| s.properties.get(dir_name(d)))
                    .is_some_and(|v| v == "true")
            };
            (BracketKind::Pipe, straight_pipe_axis(open))
        }
        _ => (BracketKind::Cog, info.axis),
    };
    let Some(clicked) = Dir::from_offset(face) else {
        return true;
    };
    // `Direction.orderedByNearest(player)[0].getOpposite()`: the face toward the player.
    let toward = game.aim().map_or(clicked, |(_, look)| {
        let (i, v) = look
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(i, v)| (i, *v))
            .unwrap_or((1, -1.0));
        let axis = [Axis::X, Axis::Y, Axis::Z][i];
        Dir::from_axis(axis, v > 0.0).opposite()
    });
    let Some(new) = decor::bracket_for_click(wooden, target, kind, clicked, toward) else {
        return true;
    };
    let old = with(dim, |d| d.brackets.get(&pos).copied());
    if old == Some(new) {
        return true;
    }
    let sound = if wooden {
        "minecraft:block.scaffolding.place"
    } else {
        "minecraft:block.netherite_block.place"
    };
    game.audio.play(sound, Some(center(pos)), 0.75, 1.0);
    with(dim, |d| {
        d.brackets.insert(pos, new);
        d.shown.push(pos);
    });
    girders_around(dim, pos);
    mark_unsaved(game, pos);
    if !game.creative {
        use_one(game);
        if let Some(b) = old {
            game.native_give_items(b.block(), 1);
        }
    }
    true
}

/// `IWrenchableWithBracket.onWrenched`: a wrench takes a shaft's bracket off first.
pub(super) fn wrench_bracket(game: &mut Game<'_>, pos: [i32; 3]) -> bool {
    let Some(dim) = dimension_index(game.dim.id()) else {
        return false;
    };
    let Some(b) = with(dim, |d| d.brackets.remove(&pos)) else {
        return false;
    };
    with(dim, |d| d.shown.push(pos));
    girders_around(dim, pos);
    mark_unsaved(game, pos);
    if !game.creative {
        game.native_give_items(b.block(), 1);
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Clicks

/// A click on one of these blocks (`useItemOn` / `useWithoutItem`); true when it was theirs.
pub(super) fn use_block(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) -> bool {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some((block, info)) = block_of(state.0) else {
        return false;
    };
    let back = sneaking(game);
    let empty_hand = held_stack(game).is_none();
    match block {
        Block::Placard => {
            // Sneaking with an item: the item's use; else the lever's template must not flip.
            if back && !empty_hand {
                return true;
            }
            *canceled = true;
            if !back {
                use_placard(game, dim, pos, &info);
            }
            true
        }
        Block::Clipboard => {
            if back && !empty_hand {
                return true;
            }
            *canceled = true;
            // `breakAndCollect` when sneaking; the pages' screen otherwise.
            if back {
                collect_clipboard(game, dim, pos);
            } else {
                let stack = with(dim, |d| d.clipboards.get(&pos).cloned())
                    .unwrap_or_else(|| ItemStack::new("create:clipboard", 1));
                super::create_clipboard::open(game, Some(pos), &stack);
            }
            true
        }
        Block::DeskBell => {
            // `useWithoutItem`: not when sneaking with an item in hand.
            if back && !empty_hand {
                return true;
            }
            *canceled = true;
            ring_desk_bell(game, dim, pos);
            true
        }
        Block::CreativeCrate => {
            use_crate(game, dim, pos, face, canceled);
            true
        }
        Block::ItemHatch => {
            use_hatch(game, dim, pos, &info, canceled);
            true
        }
        _ if game
            .inventory
            .get(game.selected)
            .and_then(|s| s.as_ref())
            .is_some_and(|s| s.item.ends_with("_table_cloth"))
            && is_table_cloth(game, state) =>
        {
            place_table_cloth(game, pos, face, canceled);
            true
        }
        _ => false,
    }
}

fn is_table_cloth(game: &Game<'_>, state: BlockStateId) -> bool {
    game.pack
        .assets
        .registry
        .state(state)
        .is_some_and(|s| s.name.starts_with("create:") && s.name.ends_with("_table_cloth"))
}

/// `PlacardBlock.useItemOn`, not sneaking.
fn use_placard(game: &mut Game<'_>, dim: usize, pos: [i32; 3], info: &StateInfo) {
    let held = held_stack(game);
    let shown = with(dim, |d| d.placards.get(&pos).cloned());
    let same = match (&held, &shown) {
        (Some(a), Some(b)) => super::create_contraption_body::same_stack(a, b),
        _ => false,
    };
    let c = center(pos);
    match decor::use_placard(
        false,
        game.may_build(),
        held.is_none(),
        shown.is_none(),
        info.powered,
        same,
    ) {
        decor::PlacardUse::Put => {
            let Some(mut one) = held else { return };
            one.count = 1;
            game.audio
                .play("minecraft:entity.item_frame.add_item", Some(c), 1.0, 1.0);
            with(dim, |d| {
                d.placards.insert(pos, one);
                d.shown.push(pos);
            });
            use_one(game);
            mark_unsaved(game, pos);
        }
        decor::PlacardUse::Pulse => {
            // AllSoundEvents.CONFIRM: the note block's bell at .5, .8.
            game.audio
                .play("minecraft:block.note_block.bell", Some(c), 0.5, 0.8);
            let dimension = game.dim.id().to_owned();
            game.native_set_block_property_with(
                pos,
                "powered",
                "true",
                super::native_hooks::flags::UPDATE_ALL,
            );
            game.native_update_signal_neighbors(&dimension, pos);
            let due = game.world.game_time + i64::from(decor::PLACARD_PULSE);
            with(dim, |d| d.timers.push((pos, due, Timer::Placard)));
        }
        // AllSoundEvents.DENY: the note block's bass at 1, .5.
        decor::PlacardUse::Deny => {
            game.audio
                .play("minecraft:block.note_block.bass", Some(c), 1.0, 0.5)
        }
        decor::PlacardUse::Fail | decor::PlacardUse::Pass => {}
    }
}

/// `DeskBellBlock.useWithoutItem`: its ding, `powered` for 20 ticks.
fn ring_desk_bell(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    // AllSoundEvents.DESK_BELL_USE: Create's own sound, from the content mod's sounds.
    game.audio
        .play("create:desk_bell", Some(center(pos)), 1.0, 1.0);
    let dimension = game.dim.id().to_owned();
    game.native_set_block_property_with(
        pos,
        "powered",
        "true",
        super::native_hooks::flags::UPDATE_ALL,
    );
    game.native_update_signal_neighbors(&dimension, pos);
    let due = game.world.game_time + i64::from(decor::DESK_BELL_TICKS);
    with(dim, |d| {
        d.timers.retain(|t| !(t.0 == pos && t.2 == Timer::Bell));
        d.timers.push((pos, due, Timer::Bell));
    });
}

/// `FilteringBehaviour` on the creative crate's top: an item sets its supply, an empty hand
/// clears it.
fn use_crate(game: &mut Game<'_>, dim: usize, pos: [i32; 3], face: [i32; 3], canceled: &mut bool) {
    if !may_use_value_box(game) {
        return;
    }
    let (Some(_), Some(hit)) = (Dir::from_offset(face), game.pick()) else {
        return;
    };
    let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
    if !create_engine::board::hits_box(decor::CRATE_SLOT, local) {
        return;
    }
    let held = game.native_held().map(|h| h.0);
    if held.as_deref() == Some("create:wrench") {
        return;
    }
    *canceled = true;
    with(dim, |d| match held {
        Some(item) => d.supplies.insert(pos, item),
        None => d.supplies.remove(&pos),
    });
    mark_unsaved(game, pos);
    game.audio.play(
        "minecraft:entity.item_frame.add_item",
        Some(center(pos)),
        0.25,
        0.1,
    );
}

/// `ItemHatchBlock.useItemOn`: its filter slot takes the held item (`FilteringBehaviour`); else
/// the held stack (sneaking: everything outside the hotbar) goes into the inventory it faces,
/// as much as fits and the filter passes.
fn use_hatch(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    info: &StateInfo,
    canceled: &mut bool,
) {
    let Some(facing) = info.facing else {
        return;
    };
    let back = sneaking(game);
    let held = game.native_held().map(|h| h.0);
    if back && held.as_deref() == Some("create:wrench") {
        return;
    }
    *canceled = true;
    if !back && let Some(hit) = game.pick() {
        let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
        if decor::hatch_filter_hit(facing, local) && held.as_deref() != Some("create:wrench") {
            with(dim, |d| match held {
                Some(item) => d.hatch_filters.insert(pos, item),
                None => d.hatch_filters.remove(&pos),
            });
            mark_unsaved(game, pos);
            return;
        }
    }
    let target = add(pos, facing);
    let face = horizon_core::geometry::Direction::ALL[facing.opposite() as usize];
    let filter = with(dim, |d| d.hatch_filters.get(&pos).cloned());
    let mut any = false;
    for slot in decor::hatch_slots(back, game.selected) {
        let Some(stack) = game.inventory.get(slot).cloned().flatten() else {
            continue;
        };
        // `canFitInsideContainerItems`: not shulker boxes.
        if stack.item.ends_with("shulker_box") {
            continue;
        }
        if filter.as_deref().is_some_and(|f| *f != *stack.item) {
            continue;
        }
        let Some(room) = super::create_logistics::insert_room(game, dim, target, face, &stack)
        else {
            // No inventory there (`targetInv == null`).
            return;
        };
        if room == 0 {
            continue;
        }
        let n = room.min(stack.count);
        let moving = ItemStack {
            count: n,
            ..stack.clone()
        };
        if let Some(s) = game.inventory.get_mut(slot) {
            *s = (stack.count > n).then(|| ItemStack {
                count: stack.count - n,
                ..stack.clone()
            });
        }
        any = true;
        if let Some(rest) = super::create_logistics::insert_item(game, dim, target, face, moving) {
            game.native_give_items(&rest.item, rest.count);
        }
    }
    if !any {
        return;
    }
    let c = center(pos);
    // AllSoundEvents.ITEM_HATCH: a barrel opening and a netherite block placed.
    game.audio
        .play("minecraft:block.barrel.open", Some(c), 0.25, 1.4);
    game.audio
        .play("minecraft:block.netherite_block.place", Some(c), 0.75, 1.15);
    if let Some(id) = with_props(game, pos, &[("open", "true")]) {
        set_state(game, pos, id);
    }
    let due = game.world.game_time + decor::HATCH_OPEN_TICKS;
    with(dim, |d| d.timers.push((pos, due, Timer::Hatch)));
    if let Some(p) = game.native_acting_player() {
        let key = if back {
            "create.item_hatch.deposit_inventory"
        } else {
            "create.item_hatch.deposit_item"
        };
        game.native_action_bar(p.uuid, horizon_client::text::Text::translate(key, &[]));
    }
}

/// `TableClothBlock.useItemOn` with a table cloth (not sneaking, not from below): the cloth goes
/// on the nearest free side of the clicked one (`PlacementHelper`).
fn place_table_cloth(game: &mut Game<'_>, pos: [i32; 3], face: [i32; 3], canceled: &mut bool) {
    if sneaking(game) || face == [0, -1, 0] || !game.may_build() {
        return;
    }
    let Some(hit) = game.pick() else {
        return;
    };
    *canceled = true;
    let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]) - 0.5);
    let b = game.res.behaviours();
    let free = decor::table_cloth_directions(local)
        .into_iter()
        .map(|d| add(pos, d))
        .find(|p| b.is_replaceable(game.world.block(p[0], p[1], p[2])));
    let Some(at) = free else {
        return;
    };
    let Some(item) = game.native_held().map(|h| h.0) else {
        return;
    };
    let registry = game.pack.assets.registry;
    let Some(id) = registry.default_state(&item) else {
        return;
    };
    set_state(game, at, id);
    game.audio
        .play("minecraft:block.wool.place", Some(center(at)), 1.0, 0.8);
    use_one(game);
}

/// `ClipboardBlock.breakAndCollect`: the clipboard leaves the world without drops and goes to
/// the player's hand (or inventory) with its pages.
fn collect_clipboard(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    let stack = with(dim, |d| d.clipboards.remove(&pos))
        .unwrap_or_else(|| ItemStack::new("create:clipboard", 1));
    if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
        set_state(game, pos, air);
    }
    match game.inventory.get_mut(game.selected) {
        Some(slot @ None) => *slot = Some(stack),
        _ => {
            if stack.extra.is_none() {
                game.native_give_items(&stack.item, 1);
            } else {
                drop_at(game, pos, stack);
            }
        }
    }
}

/// A left click: a placard gives its item back (`PlacardBlock.attack`), a clipboard is collected.
pub(super) fn left_click_block(
    game: &mut Game<'_>,
    pos: [i32; 3],
    state: BlockStateId,
    canceled: &mut bool,
) {
    if states::index(state.0).is_none() {
        return;
    }
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    match block_of(state.0).map(|(b, _)| b) {
        Some(Block::Placard) => {
            // The click goes on (digging); the item comes back first.
            let Some(stack) = with(dim, |d| d.placards.remove(&pos)) else {
                return;
            };
            with(dim, |d| d.shown.push(pos));
            mark_unsaved(game, pos);
            game.audio.play(
                "minecraft:entity.item_frame.remove_item",
                Some(center(pos)),
                1.0,
                1.0,
            );
            if stack.extra.is_none() {
                game.native_give_items(&stack.item, stack.count);
            } else {
                drop_at(game, pos, stack);
            }
        }
        Some(Block::Clipboard) => {
            *canceled = true;
            collect_clipboard(game, dim, pos);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// Items used on blocks

/// Create's items used on a block that are this module's: a shaft on a girder, brackets, the
/// vertical gearbox and the empty blaze burner. True when the use was theirs.
pub(super) fn use_item_on(
    game: &mut Game<'_>,
    stack: &ItemStack,
    pos: [i32; 3],
    face: [i32; 3],
    claimed: &mut bool,
) -> bool {
    let item = &*stack.item;
    let state = game.world.block(pos[0], pos[1], pos[2]);
    match item {
        "create:metal_bracket" | "create:wooden_bracket" => {
            *claimed = true;
            use_bracket(game, item == "create:wooden_bracket", pos, face);
            true
        }
        "create:shaft" if block_of(state.0).is_some_and(|(b, _)| b == Block::MetalGirder) => {
            *claimed = true;
            encase_girder(game, pos, face);
            true
        }
        "create:empty_blaze_burner" if spawns_capturable(game, pos) => {
            *claimed = true;
            let at = [0, 1, 2].map(|i| f64::from(pos[i]) + 0.5);
            give_burner(game, stack, at);
            true
        }
        "create:vertical_gearbox" | "create:empty_blaze_burner" => {
            *claimed = true;
            place_item(game, item, pos, face);
            true
        }
        _ => false,
    }
}

/// `GirderBlock.useItemOn` with a shaft: the girder becomes an encased shaft.
fn encase_girder(game: &mut Game<'_>, pos: [i32; 3], face: [i32; 3]) {
    let registry = game.pack.assets.registry;
    let Some(s) = registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .cloned()
    else {
        return;
    };
    let get = |k: &str| {
        s.properties
            .get(k)
            .cloned()
            .unwrap_or_else(|| "false".into())
    };
    let clicked = Dir::from_offset(face).unwrap_or(Dir::Up);
    let axis = decor::encased_axis(get("x") == "true", clicked);
    let Some(id) = state_with(
        registry,
        "create:metal_girder_encased_shaft",
        &[
            ("axis", axis_name(axis)),
            ("top", &get("top")),
            ("bottom", &get("bottom")),
            ("waterlogged", &get("waterlogged")),
        ],
    ) else {
        return;
    };
    set_state(game, pos, id);
    game.audio.play(
        "minecraft:block.netherite_block.hit",
        Some(center(pos)),
        0.5,
        1.25,
    );
    use_one(game);
}

/// `VerticalGearboxItem` and `BlazeBurnerBlockItem.empty` place their block as a block item does
/// (into the clicked block when it can be replaced, else beside it): a gearbox on Create's axis,
/// an empty blaze burner facing the player.
fn place_item(game: &mut Game<'_>, item: &str, pos: [i32; 3], face: [i32; 3]) {
    if !game.may_build() {
        return;
    }
    let b = game.res.behaviours();
    let at = if b.is_replaceable(game.world.block(pos[0], pos[1], pos[2])) {
        pos
    } else {
        [pos[0] + face[0], pos[1] + face[1], pos[2] + face[2]]
    };
    if !b.is_replaceable(game.world.block(at[0], at[1], at[2])) {
        return;
    }
    let registry = game.pack.assets.registry;
    let player = decor::horizontal_facing(game.yaw);
    let (id, sound) = if item == "create:vertical_gearbox" {
        let shafts: Vec<Dir> = [Dir::South, Dir::West, Dir::North, Dir::East]
            .into_iter()
            .filter(|d| {
                dimension_index(game.dim.id()).is_some_and(|dim| {
                    with_world(|w| w.has_shaft_towards(dim, add(at, *d), d.opposite()))
                        == Some(true)
                })
            })
            .collect();
        let axis = decor::vertical_gearbox_axis(&shafts, player);
        (
            state_with(registry, "create:gearbox", &[("axis", axis_name(axis))]),
            "minecraft:block.wood.place",
        )
    } else {
        // `BlazeBurnerBlock.getStateForPlacement`: no blaze, facing the player.
        (
            state_with(
                registry,
                "create:blaze_burner",
                &[("blaze", "none"), ("facing", dir_name(player.opposite()))],
            ),
            "minecraft:block.metal.place",
        )
    };
    let Some(id) = id else {
        return;
    };
    set_state(game, at, id);
    game.audio.play(sound, Some(center(at)), 1.0, 0.8);
    use_one(game);
}

/// `BlazeBurnerBlockItem.interactLivingEntity`: an empty blaze burner used on a blaze
/// (`create:blaze_burner_capturable`) catches it.
pub(super) fn interact_entity(
    game: &mut Game<'_>,
    entity: i32,
    kind: &str,
    stack: Option<&ItemStack>,
    claimed: &mut bool,
) {
    if kind != "minecraft:blaze" || stack.is_none_or(|s| &*s.item != "create:empty_blaze_burner") {
        return;
    }
    let Some((dimension, at)) = game
        .native_acting_player()
        .map(|p| (p.dimension, p.position))
    else {
        return;
    };
    *claimed = true;
    // `spawnCaptureEffects` at the blaze; here beside the player who caught it.
    if let Some(stack) = stack {
        give_burner(game, stack, at);
    }
    game.native_remove_mob(&dimension, entity as u32);
}

/// `BlazeBurnerBlockItem.useOn`: whether the spawner at `pos` spawns a capturable mob
/// (`create:blaze_burner_capturable`), among its potentials or, with none, its next spawn.
fn spawns_capturable(game: &Game<'_>, pos: [i32; 3]) -> bool {
    use horizon_client::chunk::block_entity::Next;
    use horizon_client::mobs::spawner::SpawnDataExt;
    let Some(horizon_client::containers::BlockEntity::Spawner(spawner)) =
        game.world.block_entities.get(&(pos[0], pos[1], pos[2]))
    else {
        return false;
    };
    let blaze = |d: &horizon_client::chunk::block_entity::SpawnData| {
        d.id().as_deref() == Some("minecraft:blaze")
    };
    if spawner.potentials.is_empty() {
        return matches!(&spawner.next, Some(Next::Own(d)) if blaze(d));
    }
    spawner.potentials.iter().any(|(d, _)| blaze(d))
}

/// `spawnCaptureEffects` (its sounds) and `giveBurnerItemTo`: the held stack shrinks, the burner
/// goes to the hand or the inventory.
fn give_burner(game: &mut Game<'_>, stack: &ItemStack, at: [f64; 3]) {
    game.audio
        .play("minecraft:entity.blaze.hurt", Some(at), 0.25, 0.75);
    game.audio
        .play("minecraft:block.fire.extinguish", Some(at), 0.5, 0.75);
    let single = stack.count <= 1;
    if !game.creative {
        use_one(game);
    }
    if single && !game.creative {
        if let Some(slot) = game.inventory.get_mut(game.selected) {
            *slot = Some(ItemStack::new("create:blaze_burner", 1));
        }
    } else {
        game.native_give_items("create:blaze_burner", 1);
    }
}

// ---------------------------------------------------------------------------------------------
// Placement

/// A block of these placed from an item: Create's placement state, and what it keeps of the
/// item (a clipboard's pages).
pub(super) fn place_block(
    game: &mut Game<'_>,
    stack: &ItemStack,
    pos: [i32; 3],
    state: BlockStateId,
) {
    if states::index(state.0).is_none() {
        return;
    }
    let Some((block, info)) = block_of(state.0) else {
        return;
    };
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    let clicked = clicked_face(game);
    let registry = game.pack.assets.registry;
    let face = registry
        .state(state)
        .and_then(|s| s.properties.get("face").cloned())
        .unwrap_or_default();
    match block {
        Block::MetalGirder => {
            let g = decor::placement(
                clicked,
                |d| side(game, pos, d),
                |d| track_beside(game, pos, d),
            );
            set_girder(game, pos, g);
        }
        Block::Placard | Block::Clipboard => {
            let placard = block == Block::Placard;
            if let Some(f) = info.facing {
                let facing = decor::placed_facing(placard, &face, f);
                let mut props = vec![("facing", dir_name(facing))];
                if !placard {
                    props.push(("written", flag(stack.extra.is_some())));
                    with(dim, |d| {
                        d.clipboards.insert(
                            pos,
                            ItemStack {
                                count: 1,
                                ..stack.clone()
                            },
                        )
                    });
                    mark_unsaved(game, pos);
                }
                if let Some(id) = with_props(game, pos, &props) {
                    set_state(game, pos, id);
                }
            }
        }
        // `DeskBellBlock.getStateForPlacement`: facing the clicked face.
        Block::DeskBell => {
            if let Some(id) = with_props(game, pos, &[("facing", dir_name(clicked))]) {
                set_state(game, pos, id);
            }
        }
        // `ItemHatchBlock.getStateForPlacement`: only on a side, facing into it.
        Block::ItemHatch => {
            if clicked.axis() == Axis::Y {
                refuse(game, pos, &stack.item);
            } else if let Some(id) = with_props(
                game,
                pos,
                &[("facing", dir_name(clicked.opposite())), ("open", "false")],
            ) {
                set_state(game, pos, id);
            }
        }
        // `CardboardBlock.getStateForPlacement`: the player's horizontal axis.
        Block::CardboardBlock | Block::BoundCardboardBlock => {
            let axis = decor::horizontal_facing(game.yaw).axis();
            if let Some(id) = with_props(game, pos, &[("axis", axis_name(axis))]) {
                set_state(game, pos, id);
            }
        }
        // `LargeWaterWheelBlock.getStateForPlacement`: the eight blocks around it in its plane
        // must be free; `extension` beside a wheel on its negative side.
        Block::LargeWaterWheel => {
            let Some(axis) = info.axis else { return };
            let b = game.res.behaviours();
            let free = create_engine::waterwheel::plane(axis).into_iter().all(|o| {
                let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
                b.is_replaceable(game.world.block(p[0], p[1], p[2]))
            });
            if !free {
                refuse(game, pos, &stack.item);
                if let Some(p) = game.native_acting_player() {
                    let line = horizon_client::text::Text::translate(
                        "create.large_water_wheel.not_enough_space",
                        &[],
                    );
                    game.native_action_bar(p.uuid, line);
                }
            }
        }
        Block::PeculiarBell => with(dim, |d| d.bell_checks.push(pos)),
        // `NozzleBlock.getStateForPlacement`: facing the clicked face; `canSurvive` (checked
        // before a placement): only on the front of an encased fan facing the same way.
        Block::Nozzle => {
            let o = clicked.offset();
            let fan = game
                .world
                .block(pos[0] - o[0], pos[1] - o[1], pos[2] - o[2]);
            let behind = block_of(fan.0)
                .filter(|(b, _)| *b == Block::EncasedFan)
                .and_then(|(_, i)| i.facing);
            if !create_engine::nozzle::survives(clicked, behind) {
                refuse(game, pos, &stack.item);
            } else if let Some(id) = with_props(game, pos, &[("facing", dir_name(clicked))]) {
                set_state(game, pos, id);
            }
        }
        _ => {}
    }
}

/// The clicked face of the placement in this tick (`BlockPlaceContext.getClickedFace`).
fn clicked_face(game: &Game<'_>) -> Dir {
    CLICKED
        .get()
        .filter(|c| c.0 == game.world.game_time)
        .and_then(|c| Dir::from_offset(c.1))
        .unwrap_or(Dir::Up)
}

/// `getStateForPlacement` answered null: the block goes and the item comes back.
fn refuse(game: &mut Game<'_>, pos: [i32; 3], item: &str) {
    if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
        set_state(game, pos, air);
    }
    if !game.creative {
        game.native_give_items(item, 1);
    }
}

// ---------------------------------------------------------------------------------------------
// Signals

/// The desk bell's signal (hot: the block_signal hook after its range test).
pub(super) fn block_signal(state: u32, side: Dir, direct: bool) -> Option<i32> {
    let (block, info) = WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        w.as_ref()?.table.get(state).map(|(b, i)| (b, *i))
    })?;
    (block == Block::DeskBell).then(|| {
        decor::desk_bell_signal(info.powered, info.facing.unwrap_or(Dir::Up), side, direct)
    })
}

/// `DeskBellBlock.isSignalSource`.
pub(super) fn is_signal_source(state: u32) -> bool {
    WORLD.with(|w| {
        w.try_borrow().ok().is_some_and(|w| {
            w.as_ref()
                .and_then(|w| w.table.get(state))
                .is_some_and(|(b, _)| b == Block::DeskBell)
        })
    })
}

// ---------------------------------------------------------------------------------------------
// The level's tick

/// After a level's tick: blocks of ours that changed, timers that ran out, girders and bells to
/// look at, the large water wheels' frames, then what clients draw. Nothing runs with nothing
/// to do.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let changes: Vec<Change> = CHANGES.with_borrow_mut(|c| {
        if c.is_empty() {
            return Vec::new();
        }
        let mine = c.iter().filter(|x| x.0 == dim).copied().collect();
        c.retain(|x| x.0 != dim);
        mine
    });
    for (_, pos, old, new) in changes {
        changed(game, dim, pos, old, new);
    }
    tick_large_wheels(game, dimension, dim);
    let now = game.world.game_time;
    let due: Vec<([i32; 3], Timer)> = DECOR.with_borrow_mut(|d| {
        let Some(d) = d.as_mut() else {
            return Vec::new();
        };
        let d = &mut d[dim];
        if d.timers.is_empty() {
            return Vec::new();
        }
        let due = d
            .timers
            .iter()
            .filter(|t| t.1 <= now)
            .map(|t| (t.0, t.2))
            .collect();
        d.timers.retain(|t| t.1 > now);
        due
    });
    for (pos, timer) in due {
        let id = game.world.block(pos[0], pos[1], pos[2]);
        let want = match timer {
            Timer::Placard => Block::Placard,
            Timer::Bell => Block::DeskBell,
            Timer::Hatch => Block::ItemHatch,
        };
        if block_of(id.0).map(|(b, _)| b) != Some(want) {
            continue;
        }
        let (key, signal) = if timer == Timer::Hatch {
            ("open", false)
        } else {
            ("powered", true)
        };
        game.native_set_block_property_with(
            pos,
            key,
            "false",
            super::native_hooks::flags::UPDATE_ALL,
        );
        if signal {
            game.native_update_signal_neighbors(dimension, pos);
        }
    }
    let (girders, bells) = DECOR.with_borrow_mut(|d| match d.as_mut() {
        Some(d) => (
            std::mem::take(&mut d[dim].girder_checks),
            std::mem::take(&mut d[dim].bell_checks),
        ),
        None => (Vec::new(), Vec::new()),
    });
    let mut girders = girders;
    girders.sort();
    girders.dedup();
    for pos in girders {
        reshape_girder(game, pos);
    }
    for pos in bells {
        haunt_bell(game, pos);
    }
    let shown = DECOR.with_borrow_mut(|d| match d.as_mut() {
        Some(d) => std::mem::take(&mut d[dim].shown),
        None => Vec::new(),
    });
    if !shown.is_empty() {
        let players: Vec<u128> = game
            .native_server_players()
            .into_iter()
            .filter(|p| p.dimension == dimension)
            .map(|p| p.uuid)
            .collect();
        send(game, &players, dim, &shown);
    }
}

/// A block of ours placed, changed or removed.
fn changed(game: &mut Game<'_>, dim: usize, pos: [i32; 3], old: u32, new: u32) {
    let (was, is) = (block_of(old).map(|x| x.0), block_of(new).map(|x| x.0));
    let registry = game.pack.assets.registry;
    // `AbstractSimpleShaftBlock.onRemove`: any other state but a waterlogging change drops its
    // bracket.
    if was.is_some_and(bracketed) {
        let only_water = match (
            registry.state(BlockStateId(old)),
            registry.state(BlockStateId(new)),
        ) {
            (Some(a), Some(b)) => {
                a.name == b.name
                    && a.properties
                        .iter()
                        .all(|(k, v)| k == "waterlogged" || b.properties.get(k) == Some(v))
            }
            _ => false,
        };
        if !only_water && let Some(b) = with(dim, |d| d.brackets.remove(&pos)) {
            with(dim, |d| d.shown.push(pos));
            girders_around(dim, pos);
            mark_unsaved(game, pos);
            drop_at(game, pos, ItemStack::new(b.block(), 1));
        }
    }
    if was == is {
        return;
    }
    match was {
        // `PlacardBlock.onRemove`: its item drops.
        Some(Block::Placard) => {
            if let Some(s) = with(dim, |d| d.placards.remove(&pos)) {
                with(dim, |d| d.shown.push(pos));
                drop_at(game, pos, s);
                mark_unsaved(game, pos);
            }
            with(dim, |d| d.timers.retain(|t| t.0 != pos));
        }
        // ClipboardBlock: no loot; the clipboard drops with its pages.
        Some(Block::Clipboard) => {
            if let Some(s) = with(dim, |d| d.clipboards.remove(&pos)) {
                drop_at(game, pos, s);
                mark_unsaved(game, pos);
            }
        }
        Some(Block::CreativeCrate) => {
            if with(dim, |d| d.supplies.remove(&pos)).is_some() {
                mark_unsaved(game, pos);
            }
        }
        Some(Block::ItemHatch) => {
            if with(dim, |d| d.hatch_filters.remove(&pos)).is_some() {
                mark_unsaved(game, pos);
            }
        }
        _ => {}
    }
    // A girder joins what is beside it when it is placed by other means than an item.
    if matches!(
        is,
        Some(Block::MetalGirder | Block::MetalGirderEncasedShaft)
    ) {
        with(dim, |d| d.girder_checks.push(pos));
    }
}

/// `PeculiarBellBlock.tryConvert`: soul fire or a soul campfire below haunts it.
fn haunt_bell(game: &mut Game<'_>, pos: [i32; 3]) {
    let registry = game.pack.assets.registry;
    let Some(s) = registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .cloned()
    else {
        return;
    };
    if s.name != "create:peculiar_bell" {
        return;
    }
    let below = registry
        .state(game.world.block(pos[0], pos[1] - 1, pos[2]))
        .map(|b| b.name.clone())
        .unwrap_or_default();
    if !decor::haunts(&below) {
        return;
    }
    let mut h = s.clone();
    h.name = "create:haunted_bell".into();
    if let Some(id) = registry.id(&h) {
        set_state(game, pos, id);
        // AllSoundEvents.HAUNTED_BELL_CONVERT: Create's own sound.
        game.audio
            .play("create:haunted_bell_convert", Some(center(pos)), 1.0, 1.0);
    }
}

/// Large water wheels: a placed one (`onPlace`'s tick) builds its frame of structure blocks,
/// breaking without drops when one can't go; a broken one takes its frame; a broken frame block
/// breaks its wheel with drops (`WaterWheelStructuralBlock.onRemove`). `extension` follows a
/// wheel on its negative side (`updateShape`).
fn tick_large_wheels(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let Some((checks, gone, frames)) = with_world(|w| {
        let d = &mut w.dims[dim];
        if d.large_wheel_checks.is_empty()
            && d.large_wheels_gone.is_empty()
            && d.wheel_structures_gone.is_empty()
        {
            return None;
        }
        Some((
            std::mem::take(&mut d.large_wheel_checks),
            std::mem::take(&mut d.large_wheels_gone),
            std::mem::take(&mut d.wheel_structures_gone),
        ))
    })
    .flatten() else {
        return;
    };
    let registry = game.pack.assets.registry;
    let air = registry.default_state("minecraft:air");
    for (pos, axis) in gone {
        // `stillValid` no longer holds for its frame: each goes (`tick`: to air).
        for (o, f) in create_engine::waterwheel::structure(axis) {
            let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            let here = game.world.block(p[0], p[1], p[2]);
            let is_frame = block_of(here.0)
                .is_some_and(|(b, i)| b == Block::WaterWheelStructure && i.facing == Some(f));
            if is_frame && let Some(air) = air {
                set_state(game, p, air);
            }
        }
    }
    for (pos, facing) in frames {
        let p1 = add(pos, facing);
        let s1 = game.world.block(p1[0], p1[1], p1[2]).0;
        let master = match block_of(s1) {
            Some((Block::LargeWaterWheel, _)) => Some(p1),
            Some((Block::WaterWheelStructure, i)) => i.facing.map(|f| add(p1, f)).filter(|p| {
                block_of(game.world.block(p[0], p[1], p[2]).0)
                    .is_some_and(|(b, _)| b == Block::LargeWaterWheel)
            }),
            _ => None,
        };
        if let Some(m) = master {
            // `level.destroyBlock(master, true)`.
            game.native_run_command(&format!(
                "execute in {dimension} run setblock {} {} {} air destroy",
                m[0], m[1], m[2]
            ));
        }
    }
    for pos in checks {
        let id = game.world.block(pos[0], pos[1], pos[2]);
        let Some((Block::LargeWaterWheel, info)) = block_of(id.0) else {
            continue;
        };
        let Some(axis) = info.axis else { continue };
        let mut broke = false;
        for (o, f) in create_engine::waterwheel::structure(axis) {
            let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            let Some(want) = state_with(
                registry,
                "create:water_wheel_structure",
                &[("facing", dir_name(f))],
            ) else {
                continue;
            };
            let here = game.world.block(p[0], p[1], p[2]);
            if here == want {
                continue;
            }
            if !game.res.behaviours().is_replaceable(here) {
                // `level.destroyBlock(pos, false)`.
                if let Some(air) = air {
                    set_state(game, pos, air);
                }
                broke = true;
                break;
            }
            set_state(game, p, want);
        }
        if broke {
            continue;
        }
        // `updateShape` toward its negative side: an extension of the wheel there.
        let behind = add(pos, Dir::from_axis(axis, false));
        let extension = block_of(game.world.block(behind[0], behind[1], behind[2]).0)
            .is_some_and(|(b, _)| b == Block::LargeWaterWheel);
        let now = game
            .pack
            .assets
            .registry
            .state(id)
            .and_then(|s| s.properties.get("extension").cloned());
        if now.as_deref() != Some(flag(extension))
            && let Some(id) = with_props(game, pos, &[("extension", flag(extension))])
        {
            set_state(game, pos, id);
        }
    }
}

/// The stack a placed clipboard at `pos` keeps (its pages), if one is there.
pub(super) fn clipboard_stack(game: &Game<'_>, dim: usize, pos: [i32; 3]) -> Option<ItemStack> {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    (block_of(id.0)?.0 == Block::Clipboard).then(|| {
        with(dim, |d| d.clipboards.get(&pos).cloned())
            .unwrap_or_else(|| ItemStack::new("create:clipboard", 1))
    })
}

/// A placed clipboard's pages edited (`ClipboardEditPacket` on a block): its stack keeps them,
/// and its `written` state follows (`ClipboardBlockEntity.updateWrittenState`).
pub(super) fn set_clipboard_stack(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: ItemStack,
) {
    let written = stack.extra.is_some();
    with(dim, |d| d.clipboards.insert(pos, stack));
    mark_unsaved(game, pos);
    if let Some(id) = with_props(game, pos, &[("written", flag(written))])
        && id != game.world.block(pos[0], pos[1], pos[2])
    {
        set_state(game, pos, id);
    }
}

// ---------------------------------------------------------------------------------------------
// The creative crate as an inventory

/// `BottomlessItemHandler` of a creative crate at `pos`: Some(supply) when one is there.
pub(super) fn crate_supply(game: &Game<'_>, dim: usize, pos: [i32; 3]) -> Option<Option<String>> {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    (block_of(id.0)?.0 == Block::CreativeCrate)
        .then(|| with(dim, |d| d.supplies.get(&pos).cloned()))
}

/// `BottomlessItemHandler.extractItem` for whoever takes from a creative crate: its supply, up to
/// a stack, if `test` passes it.
pub(super) fn crate_extract(
    game: &Game<'_>,
    supply: Option<&str>,
    test: impl Fn(&str) -> bool,
    amount: u32,
) -> Option<ItemStack> {
    let max = u32::from(super::create_logistics::stack_limit(game, supply?));
    let (item, n) = decor::crate_extract(supply.filter(|s| test(s)), amount, max)?;
    Some(ItemStack::new(item, n))
}

/// A hopper gives to the block at `pos` (Horizon API 69): a creative crate takes everything
/// (`BottomlessItemHandler.insertItem`).
pub(super) fn item_handler_insert(
    game: &Game<'_>,
    pos: [i32; 3],
    stack: &mut Option<ItemStack>,
    handled: &mut bool,
) {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    if states::index(state).is_none() {
        return;
    }
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    if crate_supply(game, dim, pos).is_some() {
        *handled = true;
        *stack = None;
    }
}

/// A hopper takes from the block at `pos` (Horizon API 69): a creative crate gives its supply
/// (`BottomlessItemHandler.extractItem`), the same whether simulated or not.
pub(super) fn item_handler_extract(
    game: &Game<'_>,
    pos: [i32; 3],
    amount: u32,
    taken: &mut Option<ItemStack>,
    handled: &mut bool,
) {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    if states::index(state).is_none() {
        return;
    }
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    if let Some(supply) = crate_supply(game, dim, pos) {
        *handled = true;
        *taken = crate_extract(game, supply.as_deref(), |_| true, amount);
    }
}

// ---------------------------------------------------------------------------------------------
// Clients

/// What clients draw at `list` (each block's bracket, placard item, or nothing) to `players`.
fn send(game: &mut Game<'_>, players: &[u128], dim: usize, list: &[[i32; 3]]) {
    let mut shown = Vec::new();
    for &pos in list {
        let (bracket, item) = with(dim, |d| {
            (
                d.brackets.get(&pos).copied(),
                d.placards.get(&pos).map(|s| s.item.to_string()),
            )
        });
        let text = bracket.map(|b| {
            format!(
                "{}[axis_along_first={},facing={},type={}]",
                b.block(),
                b.along_first,
                dir_name(b.facing),
                b.kind.name()
            )
        });
        shown.push(decor::Shown {
            pos,
            kind: 0,
            text: text.unwrap_or_default(),
        });
        shown.push(decor::Shown {
            pos,
            kind: 1,
            text: item.unwrap_or_default(),
        });
    }
    for chunk in shown.chunks(64) {
        let m = decor::encode(chunk);
        for &p in players {
            let _ = game.native_net_send_to_player(p, decor::CHANNEL, &m);
        }
    }
}

/// A client joined: everything of ours drawn in its dimension.
pub(super) fn sync(game: &mut Game<'_>, player: u128, dim: usize) {
    let list: Vec<[i32; 3]> = with(dim, |d| {
        d.brackets
            .keys()
            .chain(d.placards.keys())
            .copied()
            .collect()
    });
    if !list.is_empty() {
        send(game, &[player], dim, &list);
    }
}

/// On a client: a bracket drawn as its block's model (still), a placard's item as
/// `PlacardRenderer` poses it.
pub(super) fn from_server(game: &mut Game<'_>, data: &[u8]) {
    let registry = game.pack.assets.registry;
    for s in decor::decode(data) {
        let key = super::create_logistics::depot_key(s.pos, 110 + s.kind, 0);
        game.native_part_clear(key);
        if s.text.is_empty() {
            continue;
        }
        if s.kind == 0 {
            if let Some(id) = state_of_text(registry, &s.text)
                && let Some(model) = game.native_part_model(BlockStateId(id))
            {
                game.native_part_set(key, model, s.pos, 1, 0.0, 0.0);
            }
            continue;
        }
        let Some(st) = registry
            .state(game.world.block(s.pos[0], s.pos[1], s.pos[2]))
            .cloned()
        else {
            continue;
        };
        let face = st.properties.get("face").cloned().unwrap_or_default();
        let facing = st
            .properties
            .get("facing")
            .and_then(|f| dir_named(f))
            .unwrap_or(Dir::North);
        let block_item = game.native_item_uses_block_light(&s.text).unwrap_or(false);
        let pose = decor::placard_pose(&face, facing, block_item);
        if let Some(model) = game.native_part_item_model(&s.text, pose) {
            game.native_part_set(key, model, s.pos, 1, 0.0, 0.0);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Saving

/// A column is saved: its brackets, placards' items, crates' supplies, hatches' filters and
/// clipboards, as `create_decor`.
pub(super) fn chunk_data_save(dimension: &str, chunk: [i32; 2], data: &mut Compound) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if DECOR.with_borrow(|d| d.is_none()) {
        return;
    }
    let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let pos_tag = |p: [i32; 3]| Tag::IntArray(p.to_vec());
    let body = super::create_package_body::to_nbt;
    let lists = with(dim, |d| {
        let mut brackets: Vec<_> = d.brackets.iter().filter(|(p, _)| inside(p)).collect();
        brackets.sort_by_key(|(p, _)| **p);
        let brackets: Vec<Tag> = brackets
            .into_iter()
            .map(|(p, b)| {
                Tag::IntArray(vec![
                    p[0],
                    p[1],
                    p[2],
                    i32::from(b.wooden),
                    b.facing as i32,
                    i32::from(b.along_first),
                    match b.kind {
                        BracketKind::Pipe => 0,
                        BracketKind::Cog => 1,
                        BracketKind::Shaft => 2,
                    },
                ])
            })
            .collect();
        let stacks = |m: &HashMap<[i32; 3], ItemStack>| -> Vec<Tag> {
            let mut v: Vec<_> = m.iter().filter(|(p, _)| inside(p)).collect();
            v.sort_by_key(|(p, _)| **p);
            v.into_iter()
                .map(|(p, s)| Tag::compound([("Pos", pos_tag(*p)), ("Item", body(s))]))
                .collect()
        };
        let names = |m: &HashMap<[i32; 3], String>| -> Vec<Tag> {
            let mut v: Vec<_> = m.iter().filter(|(p, _)| inside(p)).collect();
            v.sort_by_key(|(p, _)| **p);
            v.into_iter()
                .map(|(p, s)| {
                    Tag::compound([
                        ("Pos", pos_tag(*p)),
                        ("Item", Tag::String(s.as_str().into())),
                    ])
                })
                .collect()
        };
        [
            ("brackets", brackets),
            ("placards", stacks(&d.placards)),
            ("clipboards", stacks(&d.clipboards)),
            ("supplies", names(&d.supplies)),
            ("hatches", names(&d.hatch_filters)),
        ]
    });
    if lists.iter().all(|(_, l)| l.is_empty()) {
        data.remove(&KEY.into());
        return;
    }
    data.insert(
        KEY.into(),
        Tag::compound(lists.map(|(k, l)| (k, Tag::List(l)))),
    );
}

fn list(data: &Compound, key: &str) -> Vec<Tag> {
    match data.get(&key.into()) {
        Some(Tag::List(l)) => l.to_vec(),
        _ => Vec::new(),
    }
}

fn pos_of(c: &Compound) -> Option<[i32; 3]> {
    match c.get(&"Pos".into()) {
        Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
        _ => None,
    }
}

/// A column joined a level: what it saved of ours back.
pub(super) fn chunk_load(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let Some(data) = Game::native_chunk_data(chunk, KEY) else {
        return;
    };
    let body = super::create_package_body::from_nbt;
    with(dim, |d| {
        for t in list(data, "brackets") {
            let Tag::IntArray(v) = t else { continue };
            if v.len() != 7 {
                continue;
            }
            let kind = [BracketKind::Pipe, BracketKind::Cog, BracketKind::Shaft]
                [v[6].clamp(0, 2) as usize];
            d.brackets.insert(
                [v[0], v[1], v[2]],
                Bracket {
                    wooden: v[3] != 0,
                    facing: Dir::ALL[v[4].clamp(0, 5) as usize],
                    along_first: v[5] != 0,
                    kind,
                },
            );
            d.shown.push([v[0], v[1], v[2]]);
        }
        for (key, placards) in [("placards", true), ("clipboards", false)] {
            for t in list(data, key) {
                let Tag::Compound(c) = t else { continue };
                let (Some(p), Some(s)) = (pos_of(&c), c.get(&"Item".into()).and_then(body)) else {
                    continue;
                };
                if placards {
                    d.placards.insert(p, s);
                    d.shown.push(p);
                } else {
                    d.clipboards.insert(p, s);
                }
            }
        }
        for (key, supplies) in [("supplies", true), ("hatches", false)] {
            for t in list(data, key) {
                let Tag::Compound(c) = t else { continue };
                let item = match c.get(&"Item".into()) {
                    Some(Tag::String(s)) => s.to_string_lossy(),
                    _ => continue,
                };
                let Some(p) = pos_of(&c) else { continue };
                if supplies {
                    d.supplies.insert(p, item);
                } else {
                    d.hatch_filters.insert(p, item);
                }
            }
        }
    });
}

/// A column left a level: what it held of ours is forgotten (saved with it).
pub(super) fn chunk_unload(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if DECOR.with_borrow(|d| d.is_none()) {
        return;
    }
    let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) != (chunk.x, chunk.z);
    with(dim, |d| {
        d.brackets.retain(|p, _| inside(p));
        d.placards.retain(|p, _| inside(p));
        d.clipboards.retain(|p, _| inside(p));
        d.supplies.retain(|p, _| inside(p));
        d.hatch_filters.retain(|p, _| inside(p));
        d.timers.retain(|t| inside(&t.0));
    });
}
