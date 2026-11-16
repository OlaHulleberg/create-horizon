//! The copycats' Parts: their blocks' clicks and placement, the copycat items' `setPlacedBy`,
//! and the block entity that draws the material.

use super::super::persist;
use super::super::server::ServerLevel;
use super::super::states;
use super::{
    MATERIAL, SPECIAL, body, center, copycat_block, set_material, shape_of, shown, with, with_prop,
};
use create_engine::Block;
use create_engine::copycat::{self as cc, Drawn, Shape, Special, Use};
use create_engine::kinetics::Dir;
use mod_api::{
    BlockEntityPart, BlockEntitySubmitEvent, BlockEntityUpdateTagEvent, BlockPart, BlockState,
    ClientHost, Compound, Direction, Hand, InteractionResult, ItemPart, ItemStack, LevelHost,
    OnBlockPlacedByEvent, PlayerRef, ServerHost, SoundSource, StateForPlacementEvent, Tag,
    UseItemOnEvent,
};

fn dir(d: Direction) -> Dir {
    Dir::ALL[d as usize]
}

/// One of `stack`, as a copycat keeps it (`consumedItem`).
fn one(stack: &ItemStack) -> ItemStack {
    let mut one = stack.clone();
    let _ = one.set_count(1);
    one
}

/// One of the stack in `player`'s `hand` used up (not in creative).
fn use_one(host: &mut dyn ServerHost, player: PlayerRef, hand: Hand, stack: &ItemStack) {
    if player.is_creative(&*host) == Some(true) {
        return;
    }
    if let Some(living) = host.living(player.entity()) {
        let _ = living.set_hand_item(host, hand, stack.clone().shrink(1));
    }
}

fn play(lv: &mut ServerLevel<'_>, sound: &str, pos: [i32; 3], volume: f32, pitch: f32) {
    lv.level.play_sound(
        &mut *lv.host,
        sound,
        center(pos),
        SoundSource::Blocks,
        volume,
        pitch,
    );
}

/// The copycats' placement helpers and `PlacementOffset.placeInWorld`. A panel
/// (`CopycatPanelBlock.PlacementHelper`): on the free side nearest the hit across its facing,
/// facing the same way; the click is taken even with no place. A step (`CopycatStepBlock
/// .PlacementHelper`, a `PoleHelper` along its facing turned clockwise): past the steps in line
/// with it, with its facing and half; with no place the click goes on as usual. The new copycat
/// holds no material.
fn helper(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    click: &mod_api::BlockClick<'_>,
    stack: &ItemStack,
    state: BlockState,
    shape: Shape,
) -> Option<InteractionResult> {
    let Some(at) = target(lv, click.pos, click.hit.point, shape) else {
        // A panel takes the click with no place; a step lets it go on.
        return matches!(shape, Shape::Panel(_)).then_some(InteractionResult::Success);
    };
    let water = lv
        .block_state(at)
        .and_then(|s| lv.host.block_state_string(s))
        .is_some_and(|t| t.starts_with("minecraft:water"));
    let base = lv.host.block_state_default(state.block())?;
    let mut next = base;
    let facts = mod_api::HostBlockStates::block_states(&*lv.host);
    let mut props: Vec<(&str, String)> = vec![("waterlogged", water.to_string())];
    for k in ["facing", "half"] {
        if let Ok(Some(v)) = state.property(&facts, k) {
            props.push((k, v.to_owned()));
        }
    }
    for (k, v) in &props {
        if let Some(n) = with_prop(&*lv.host, next, k, v) {
            next = n;
        }
    }
    lv.set_state(at, next, crate::glue::server::update_all());
    lv.place_sound(at);
    use_one(&mut *lv.host, player, click.hand, stack);
    Some(InteractionResult::Success)
}

/// The copycat helpers' `getOffset`: where a copycat item clicked at `point` on the copycat at
/// `pos` (`shape`) goes.
fn target(lv: &ServerLevel<'_>, pos: [i32; 3], point: [f64; 3], shape: Shape) -> Option<[i32; 3]> {
    let hit = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]) - 0.5);
    let step_axis = |f: Dir| match f {
        Dir::North | Dir::South => create_engine::kinetics::Axis::X,
        _ => create_engine::kinetics::Axis::Z,
    };
    match shape {
        Shape::Panel(facing) => create_engine::place::ordered_by_distance(hit, |d| {
            d.axis() != facing.axis() && lv.free(add(pos, d))
        })
        .first()
        .map(|d| add(pos, *d)),
        Shape::Step { facing, .. } => {
            let axis = step_axis(facing);
            let pole = |p: [i32; 3]| {
                lv.block_state(p).is_some_and(|s| {
                    matches!(shape_of(&*lv.host, s), Some(Shape::Step { facing: f, .. })
                        if step_axis(f) == axis)
                })
            };
            create_engine::place::pole_helper(pos, axis, hit, pole, |p| lv.free(p))
        }
    }
}

/// The copycat helpers' target for the indicator: a panel item on a panel, a step on a step,
/// and what it takes of the clicked one (its facing, a step's half too), for its ghost.
pub(crate) fn helper_target(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    point: [f64; 3],
    item: &str,
) -> Option<crate::glue::interaction::HelperTarget> {
    let shape = shape_of(&*lv.host, lv.block_state(pos)?)?;
    let (own, props) = match shape {
        Shape::Panel(f) => ("create:copycat_panel", vec![("facing", cc::dir_name(f))]),
        Shape::Step { facing, top } => (
            "create:copycat_step",
            vec![
                ("facing", cc::dir_name(facing)),
                ("half", if top { "top" } else { "bottom" }),
            ],
        ),
    };
    let props = props.into_iter().map(|(k, v)| (k, v.to_owned())).collect();
    (item == own)
        .then(|| target(lv, pos, point, shape))
        .flatten()
        .map(|at| (at, props))
}

fn add(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

/// The copycat panel's and step's block moments.
pub(super) struct CopycatBlock;

#[mod_api::horizon_part]
impl BlockPart for CopycatBlock {
    /// `CopycatBlock.useItemOn`: a block it takes becomes its material (one consumed, none in
    /// creative), the same block again turns it (`cycleMaterial`); anything else passes to the
    /// item's own use (the wrench's is `Copycats::wrench_used`).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let stack = click.stack?;
        let player = host.player(click.player)?;
        let dimension = click.dimension.key().id().to_owned();
        let mut lv = ServerLevel::new(host, &dimension)?;
        let pos = click.pos;
        if stack.item().id(&*lv.host) == "create:wrench" {
            return None;
        }
        let shape = shape_of(&*lv.host, event.state)?;
        // `CopycatPanelBlock` / `CopycatStepBlock.useItemOn`: not sneaking, its own item goes
        // beside it by the placement helper.
        let sneaking = player.entity().is_shift_key_down(&*lv.host) == Some(true);
        if !sneaking && player.may_build(&*lv.host) == Some(true) {
            let own = match shape {
                Shape::Panel(_) => "create:copycat_panel",
                Shape::Step { .. } => "create:copycat_step",
            };
            if stack.item().id(&*lv.host) == own {
                return helper(&mut lv, player, click, stack, event.state, shape);
            }
        }
        let clicked = Dir::from_offset(click.hit.face);
        let pass = Some(InteractionResult::TryWithEmptyHand);
        let Some((mut material, special)) = body::accepted(&lv, pos, shape, stack, clicked) else {
            return pass;
        };
        // `CopycatPanelBlock.prepareMaterial`: a trapdoor lies or stands as the panel does.
        if let (Shape::Panel(facing), Special::Trapdoor) = (shape, special) {
            let yaw = player.entity().rotation(&*lv.host).map_or(0.0, |r| r[0]);
            let toward = create_engine::place::horizontal_direction(yaw);
            let near_top = click.hit.point[1] - 0.5 > f64::from(pos[1]);
            for (k, v) in cc::trapdoor_material(facing, toward, near_top) {
                if let Some(next) = with_prop(&*lv.host, material, k, v) {
                    material = next;
                }
            }
        }
        let dim = lv.dim;
        let current = with(dim, |d| d.copycats.get(&pos).and_then(|c| c.material));
        let same = current.is_some_and(|c| c.block() == material.block());
        let turned = current.and_then(|c| {
            let (k, v) = body::cycle_of(&*lv.host, c)?;
            with_prop(&*lv.host, c, k, v)
        });
        match cc::use_with(true, same, current.is_some(), turned.is_some()) {
            Use::Pass => pass,
            Use::Cycle => {
                if let Some(t) = turned {
                    set_material(&mut lv, pos, t);
                    persist::mark_pos_unsaved(dim, pos);
                }
                play(
                    &mut lv,
                    "minecraft:entity.item_frame.add_item",
                    pos,
                    0.75,
                    0.95,
                );
                Some(InteractionResult::Success)
            }
            Use::Apply => {
                set_material(&mut lv, pos, material);
                with(dim, |d| {
                    d.copycats.entry(pos).or_default().item = Some(one(stack))
                });
                persist::mark_pos_unsaved(dim, pos);
                if let Some(sound) = body::place_sound(&*lv.host, material) {
                    play(&mut lv, &sound, pos, 1.0, 0.75);
                }
                use_one(&mut *lv.host, player, click.hand, stack);
                Some(InteractionResult::Success)
            }
        }
    }

    /// `getStateForPlacement`: a panel faces away from the player's view, a step their way
    /// (upper half under a block or high on a side).
    fn state_for_placement(
        &self,
        level: &dyn LevelHost,
        event: &mut StateForPlacementEvent<'_>,
    ) -> Option<Option<BlockState>> {
        let block = copycat_block(event.state.block())?;
        let c = event.context;
        let nearest = create_engine::place::ordered_by_nearest(c.yaw, c.pitch)[0];
        let horizontal = create_engine::place::horizontal_direction(c.yaw);
        // `getClickLocation().y - getClickedPos().getY()`.
        let hit_y = c.hit[1] - f64::from(c.pos.y);
        let shape = cc::placed(
            block == Block::CopycatPanel,
            nearest,
            horizontal,
            dir(c.face),
            hit_y,
        );
        let props: &[(&str, &str)] = match shape {
            Shape::Panel(f) => &[("facing", cc::dir_name(f))],
            Shape::Step { facing, top } => &[
                ("facing", cc::dir_name(facing)),
                ("half", if top { "top" } else { "bottom" }),
            ],
        };
        let mut state = event.state;
        for (k, v) in props {
            if let Some(next) = level.state_with_property(state, k, v) {
                state = next;
            }
        }
        Some(Some(state))
    }
}

/// `CopycatBlock.onWrenched`: the material comes off, its item back to the player (not in
/// creative). False when it holds none (`PASS`).
pub(super) fn wrench(lv: &mut ServerLevel<'_>, pos: [i32; 3], player: PlayerRef) -> bool {
    let dim = lv.dim;
    let Some(had) = with(dim, |d| {
        let c = d.copycats.get_mut(&pos)?;
        c.material?;
        Some(std::mem::take(c))
    }) else {
        return false;
    };
    if player.is_creative(&*lv.host) != Some(true)
        && let Some(item) = had.item
    {
        // `placeItemBackInInventory`: what doesn't fit drops at the player.
        let _ = player.give_stack(&mut *lv.host, item);
    }
    // `levelEvent(PARTICLES_DESTROY_BLOCK, pos, Block.getId(ufte.getBlockState()))`.
    if let Some(state) = lv.block_state(pos) {
        lv.destroy_effect(pos, state);
    }
    shown(dim, pos);
    persist::mark_pos_unsaved(dim, pos);
    true
}

/// The copycat items: `CopycatBlock.setPlacedBy`.
pub(super) struct CopycatItem;

#[mod_api::horizon_part]
impl ItemPart for CopycatItem {
    /// `setPlacedBy`: a block in the placer's off hand becomes the material, turned to the side
    /// nearest their view.
    fn on_block_placed_by(&self, host: &mut dyn ServerHost, event: &mut OnBlockPlacedByEvent<'_>) {
        let Some(player) = event.placer else {
            return;
        };
        let Some(dimension) = player.level(&*host).and_then(|l| l.dimension().id(&*host)) else {
            return;
        };
        let Some(living) = host.living(player.entity()) else {
            return;
        };
        let Some(offhand) = living.hand_item(&*host, Hand::Off) else {
            return;
        };
        let rotation = player.entity().rotation(&*host).unwrap_or([0.0; 2]);
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        let Some(shape) = shape_of(&*lv.host, event.state) else {
            return;
        };
        let pos = event.pos;
        let nearest = create_engine::place::ordered_by_nearest(rotation[0], rotation[1])[0];
        let Some((material, _)) = body::accepted(&lv, pos, shape, &offhand, Some(nearest)) else {
            return;
        };
        let dim = lv.dim;
        if with(dim, |d| {
            d.copycats.get(&pos).is_some_and(|c| c.material.is_some())
        }) {
            return;
        }
        set_material(&mut lv, pos, material);
        with(dim, |d| {
            d.copycats.entry(pos).or_default().item = Some(one(&offhand))
        });
        persist::mark_pos_unsaved(dim, pos);
        use_one(&mut *lv.host, player, Hand::Off, &offhand);
    }
}

/// The copycat's block entity: what it shows reaches its clients, which draw it.
pub(super) struct CopycatEntity;

#[mod_api::horizon_part]
impl BlockPart for CopycatEntity {}

/// How far the material is drawn outside the copycat's box, over the copycat's own model.
const OVER: f32 = 1.0 / 512.0;

#[mod_api::horizon_part]
impl BlockEntityPart for CopycatEntity {
    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        let nbt = &event.entity.nbt;
        let material = nbt.get(&MATERIAL.into())?.clone();
        let special = nbt.get(&SPECIAL.into()).cloned().unwrap_or(Tag::Byte(0));
        Some(Compound::from([
            (MATERIAL.into(), material),
            (SPECIAL.into(), special),
        ]))
    }

    /// `CopycatModel`: the material cut to the copycat's box, the copycat base when it holds
    /// none; a panel's trapdoor whole and its bars as the copycat bars' model.
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(shape) = shape_of(host, event.state) else {
            return;
        };
        let nbt = &event.entity.nbt;
        let text = match nbt.get(&MATERIAL.into()) {
            Some(Tag::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        };
        let special = match nbt.get(&SPECIAL.into()) {
            Some(Tag::Byte(1)) => Special::Bars,
            Some(Tag::Byte(2)) => Special::Trapdoor,
            _ => Special::None,
        };
        let material = if text.is_empty() {
            host.block_state_id(cc::BASE)
        } else {
            host.block_state_id(&text)
        };
        // `getSoundType` on this client: the material's.
        super::sound::set_client(event.pos, &text, || {
            material.and_then(|m| mod_api::HostBlockStates::block_states(host).sound(m))
        });
        let Some(material) = material else {
            return;
        };
        let draw = &mut *event.draw;
        match cc::drawn(shape, special) {
            Drawn::Whole => {
                let full = host.block_state_string(material).unwrap_or_default();
                let (_, props) = states::split(&full);
                draw.block(material.block(), &props);
            }
            Drawn::Bars(f) => {
                draw.block(
                    states::key(Block::CopycatBars),
                    &[("facing", cc::dir_name(f))],
                );
            }
            Drawn::Cut(boxes) => {
                let Some(Some(sprites)) = host.block_sprites(&[material]).pop() else {
                    return;
                };
                for [lo, hi] in boxes {
                    let from = lo.map(|v| v - OVER);
                    let to = hi.map(|v| v + OVER);
                    for side in Direction::ALL {
                        for face in &sprites.sides[side as usize] {
                            // Faces inside the material's block are not on its sides.
                            if face.inset != 0.0 {
                                continue;
                            }
                            let Some((corners, uv)) = face.quad_on(side, from, to) else {
                                continue;
                            };
                            let tint = if face.tint_index >= 0 {
                                host.block_tint_color(material, face.tint_index, event.pos)
                                    .unwrap_or(0xFF_FFFF)
                            } else {
                                0xFF_FFFF
                            };
                            draw.quad(&face.texture, corners, uv, tint & 0xFF_FFFF);
                        }
                    }
                }
            }
        }
        draw.keep();
    }
}

/// A copycat riding a moving structure: its material as it was taken (`CopycatModel`).
pub(super) fn draw_carried(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    CopycatEntity.block_entity_submit(host, event);
}
