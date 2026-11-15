//! Mechanical arms and weighted ejectors on the server: the points an arm works on and what they
//! hold (`ArmBlockEntity.tick`), the ejector's launches and landings (`EjectorBlockEntity.tick`),
//! their redstone, their saved data, and what clients draw (`create:arm`, `create:ejector`).
//!
//! An arm's points are chosen with the arm item (`ArmInteractionPointHandler`): the selection is
//! kept on the server and sent to the player's client, which outlines it while the arm is in
//! hand (and an arm's points while a wrench looks at it). Every point type of Create 6 works:
//! depots, ejectors and stations, belts, chutes, funnels, basins, millstones, crafters,
//! deployers, blaze burners, crushing wheels, packagers, saws facing up, campfires,
//! composters, jukeboxes (the arm dances while one plays) and respawn anchors. Stacks keep their
//! components through an arm or ejector (`items::key_of`), onto belts too.
//! The weighted ejector's target outline and arc are drawn as `EjectorTargetHandler` draws them.
//!
//! An arm clicked with goggles wears them; a dancing arm's body takes Create's rainbow (tinted
//! copies of its partials, made by `create-data prepare`). A charged ejector fires when a mob
//! stands on it (`block_step_on`, acted on at the level's tick) or a player
//! does (their client asks, `create:ejector_trigger`), throwing them along its arc.
//!
//! Left out: the elytra a launched player deploys.

use super::super::net;
use super::super::server::ServerLevel;
use super::super::subsystem::Subsystem;
use super::super::world::with_world;
use super::belt::{belt_segment, put_on_from_above};
use super::chute::{chute_accepts, chute_at, chute_stack, set_chute_stack};
use super::depot::depot_items;
use super::funnel::funnel_at;
use super::items::{self, face, random_angle, stack_limit, upright};
use super::{has_collision, info_at};
use create_engine::Block;
use create_engine::arm::{self, Kind, Point, Stack};
use create_engine::ejector;
use create_engine::kinetics::Dir;
use mod_api::{Compound, PlayerRef, ServerHost, Tag};
use std::collections::HashMap;

/// Mechanical arms and weighted ejectors.
pub struct Arms;

/// What a moving structure carries for an ejector (`Subsystem::carry_out`).
const CARRIED_EJECTOR: &str = "EjectorLauncher";
/// What a moving structure carries for a mechanical arm (`Subsystem::carry_out`).
pub(super) const CARRIED_ARM: &str = "Arm";

impl Subsystem for Arms {
    /// A weighted ejector's target and stack size (`EjectorBlockEntity.write`: `HorizontalDistance`,
    /// `VerticalDistance`, `MaxStackSize`) ride with it; the stacks in flight drop as it goes.
    fn carry_out(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        block: create_engine::Block,
        out: &mut Compound,
    ) {
        let dim = lv.dim;
        if block == create_engine::Block::MechanicalArm {
            // `ArmBlockEntity.write`: its points (from it), phase and held stack; taken, so
            // the stack doesn't drop as the arm goes.
            let a = with_world(|w| {
                let arms = &mut w.dims[dim].arms;
                let a = arms.arms.get(&pos).cloned()?;
                arms.removed(pos);
                Some(a)
            })
            .flatten();
            if let Some(a) = a {
                out.insert(CARRIED_ARM.into(), Tag::Compound(arm_to_tag(pos, &a)));
            }
            return;
        }
        if block != create_engine::Block::WeightedEjector {
            return;
        }
        let e = with_world(|w| w.dims[dim].ejectors.get(&pos).cloned()).flatten();
        if let Some(e) = e {
            out.insert(
                CARRIED_EJECTOR.into(),
                Tag::IntArray(vec![e.launcher.h, e.launcher.v, i32::from(e.max_stack)]),
            );
        }
    }

    fn carry_in(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        block: create_engine::Block,
        data: &Compound,
        turn: create_engine::contraption::Transform,
    ) {
        if let (Some(Tag::Compound(c)), create_engine::Block::MechanicalArm) =
            (data.get(&CARRIED_ARM.into()), block)
        {
            // Its points turned with it about where it was (`ArmBlockEntity.transform`).
            if let Some((was, mut a)) = arm_from_tag(c) {
                for pt in a.inputs.iter_mut().chain(a.outputs.iter_mut()) {
                    let o = turn.vec([0, 1, 2].map(|i| pt.pos[i] - was[i]));
                    pt.pos = [0, 1, 2].map(|i| pos[i] + o[i]);
                    pt.facing = pt.facing.map(|f| turn.dir(f));
                }
                let dim = lv.dim;
                with_world(|w| w.dims[dim].arms.saved.insert(pos, a));
            }
            return;
        }
        let Some(Tag::IntArray(v)) = data.get(&CARRIED_EJECTOR.into()) else {
            return;
        };
        let (&[h, vertical, max], create_engine::Block::WeightedEjector) = (&v[..], block) else {
            return;
        };
        let mut e = ejector::Ejector {
            launcher: ejector::Launcher::new(h, vertical),
            ..Default::default()
        };
        e.launcher.clamp(ejector::MAX_DISTANCE);
        e.max_stack = max.clamp(0, 64) as u8;
        let dim = lv.dim;
        with_world(|w| w.dims[dim].ejector_saved.insert(pos, e));
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        arms_from_nbt(lv.dim, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        arms_to_nbt(lv.dim, chunk, data);
    }

    fn speed_changed(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], _speed: f32) {
        wake_on_speed(lv.dim, pos);
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        turn_placed_ejectors(lv);
        tick_arms(lv);
        entities_on_ejectors(lv);
        tick_ejectors(lv);
    }

    fn right_click_block(
        &self,
        mut host: mod_api::HostSide,
        event: &mut mod_api::RightClickBlockEvent<'_>,
    ) -> mod_api::Flow {
        if put_on_goggles(&mut host, event) {
            return mod_api::Flow::Cancel;
        }
        let again = match &mut host {
            mod_api::HostSide::Server(h) => mod_api::HostSide::Server(&mut **h),
            mod_api::HostSide::Client(h) => mod_api::HostSide::Client(&mut **h),
        };
        match select_point(again, event) {
            mod_api::Flow::Continue => select_ejector_target(host, event),
            cancel => cancel,
        }
    }

    fn entity_place(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::EntityPlaceEvent,
    ) -> mod_api::Flow {
        arm_placed(host, event);
        ejector_placed(host, event);
        mod_api::Flow::Continue
    }

    /// `EjectorTriggerPacket`: a player's client stands on a charged ejector.
    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        if channel != super::client::EJECTOR_TRIGGER {
            return false;
        }
        let w: Vec<i32> = data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| i32::from_le_bytes(*b))
            .collect();
        let ([x, y, z], Some(at)) = (
            [w.first(), w.get(1), w.get(2)].map(|v| v.copied().unwrap_or(i32::MIN)),
            player.entity().position(&*host),
        ) else {
            return true;
        };
        let d2 = (at[0] - f64::from(x) - 0.5).powi(2)
            + (at[1] - f64::from(y) - 1.0).powi(2)
            + (at[2] - f64::from(z) - 0.5).powi(2);
        let dim = player
            .dimension(&*host)
            .and_then(|d| super::super::world::level_index(&d.to_string()));
        if let Some(dim) = dim
            && d2 < 4.0
        {
            trigger_ejector(dim, [x, y, z]);
        }
        true
    }

    fn player_joined(&self, host: &mut dyn ServerHost, player: PlayerRef) {
        let Some(level) = player.level(&*host) else {
            return;
        };
        let Some(dimension) = level.dimension().id(&*host) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        sync_arms(&mut lv, player);
    }

    fn server_message(
        &self,
        host: &mut dyn mod_api::ClientHost,
        channel: &str,
        data: &[u8],
    ) -> bool {
        if channel == POINTS {
            super::client::selection_message(data);
            return true;
        }
        if channel == EJECTOR_TARGET_CHANNEL {
            super::client::ejector_target_message(data);
            return true;
        }
        super::client::arms_message(host, channel, data)
    }

    fn render_level_stage(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
        super::client::draw_arm_points(host, event);
    }

    fn client_tick(&self, host: &mut dyn mod_api::ClientHost) {
        super::client::tick_client_arms(host);
        super::client::tick_ejector_arc(host);
        super::client::trigger_from_local_player(host);
    }

    fn register(&self, r: &mut dyn mod_api::Registrar, _cx: &mod_api::RegistrationContext) {
        super::client::register_particles(r);
    }

    fn client_reset(&self) {
        super::client::reset_arms();
    }
}

/// The arm interaction point type of `block` (`ArmInteractionPointType.getPrimaryType`, in
/// Create's registry order). A saw is one only facing up and turning (`SawType`).
pub fn point_kind(block: Block) -> Option<Kind> {
    Some(match block {
        Block::Basin => Kind::Basin,
        Block::Belt => Kind::Belt,
        // `AllBlocks.BLAZE_BURNER`: not the lit burner.
        Block::BlazeBurner => Kind::BlazeBurner,
        Block::Chute | Block::SmartChute => Kind::Chute,
        Block::MechanicalCrafter => Kind::Crafter,
        Block::CrushingWheelController => Kind::CrushingWheels,
        Block::Deployer => Kind::Deployer,
        Block::Depot | Block::WeightedEjector | Block::TrackStation => Kind::Depot,
        Block::AndesiteFunnel
        | Block::BrassFunnel
        | Block::AndesiteBeltFunnel
        | Block::BrassBeltFunnel => Kind::Funnel,
        Block::Millstone => Kind::Millstone,
        Block::Packager | Block::Repackager => Kind::Packager,
        Block::MechanicalSaw => Kind::Saw,
        _ => return None,
    })
}

/// The vanilla blocks Create's arm works with, by id (`CampfireType`: any `CampfireBlock`,
/// `ComposterType`, `JukeboxType`, `RespawnAnchorType`).
pub fn vanilla_point_kind(name: &str) -> Option<Kind> {
    Some(match name {
        "minecraft:campfire" | "minecraft:soul_campfire" => Kind::Campfire,
        "minecraft:composter" => Kind::Composter,
        "minecraft:jukebox" => Kind::Jukebox,
        "minecraft:respawn_anchor" => Kind::RespawnAnchor,
        _ => return None,
    })
}

/// The block at `pos` as an arm interaction point (`canCreatePoint`).
pub fn arm_point(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Point> {
    let Some((block, info)) = info_at(lv, pos) else {
        let kind = vanilla_point_kind(&lv.block_name(pos)?)?;
        return Some(Point {
            pos,
            kind,
            facing: None,
        });
    };
    let kind = point_kind(block)?;
    if kind == Kind::Saw {
        let dim = lv.dim;
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        if info.facing != Some(Dir::Up) || speed == 0.0 {
            return None;
        }
    }
    Some(Point {
        pos,
        kind,
        facing: info.facing,
    })
}

/// A property of the block at `pos` (`has_record`, `charge`).
fn property(lv: &ServerLevel<'_>, pos: [i32; 3], key: &str) -> Option<String> {
    let text = lv.host.block_state_string(lv.block_state(pos)?)?;
    let (_, props) = super::super::states::split(&text);
    props
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_owned())
}

/// The first stack the container at `pos` would give through `side`, looked at only.
fn peek(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    side: mod_api::Direction,
) -> Option<mod_api::ItemStack> {
    let from = items::items_at(lv, pos, Some(side))?;
    let host = &mut *lv.host;
    (0..from.size(host)).find_map(|i| {
        let r = from.resource(host, i)?;
        let n = from.amount(host, i);
        (n > 0).then(|| r.to_stack(n)).flatten()
    })
}

/// `DeployerFakePlayer`'s stand-in for an arm's use of a vanilla block: the arm's own fake
/// player.
const ARM_FAKE: (u128, &str) = (0x9e2f_aded_cafe_4ec2_c314_dad1_29ae_9720, "Mechanical Arm");

/// The arm's fake player uses `stack` on the top of the block at `pos` (a campfire's
/// `placeFood`, a jukebox's record): what is left of the stack.
fn use_on_top(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    stack: mod_api::ItemStack,
) -> Option<mod_api::ItemStack> {
    let Some(fake) = lv.host.fake_player(lv.level, ARM_FAKE.0, ARM_FAKE.1) else {
        return Some(stack);
    };
    let eye = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 1.7,
        f64::from(pos[2]) + 0.5,
    ];
    let setup = mod_api::PlayerActionSetup::new(Some(stack.clone()), eye, 0.0, 90.0, false);
    let hit = mod_api::BlockHit::new([eye[0], f64::from(pos[1]) + 1.0, eye[2]], [0, 1, 0], false);
    let action = mod_api::PlayerAction::UseOn {
        hand: mod_api::Hand::Main,
        pos,
        hit,
    };
    match fake.perform_action_scoped(&mut *lv.host, action, setup) {
        Ok(result) => result.held,
        Err(_) => Some(stack),
    }
}

/// Whether the depot at `pos` takes items: always, unless it is an ejector that is not charged
/// (`canAcceptItems`).
pub fn ejector_accepts(dim: usize, pos: [i32; 3]) -> bool {
    with_world(|w| {
        w.dims[dim]
            .ejectors
            .get(&pos)
            .is_none_or(|e| e.state == ejector::State::Charged)
    })
    .unwrap_or(true)
}

/// Whether a weighted ejector is at `pos`.
pub fn is_ejector(dim: usize, pos: [i32; 3]) -> bool {
    with_world(|w| w.dims[dim].ejectors.contains_key(&pos)) == Some(true)
}

/// A block entity's slot, as (item, count).
fn slot_at(lv: &ServerLevel<'_>, pos: [i32; 3], slot: usize) -> Option<Stack> {
    let s = items::slot(lv, pos, slot as u32)?;
    Some((items::key_of(&*lv.host, &s), s.count().min(255) as u8))
}

/// Takes up to `amount` from a block entity's slot (or only looks, `simulate`).
fn take_slot(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    slot: usize,
    amount: u8,
    simulate: bool,
) -> Option<Stack> {
    let held = items::slot(lv, pos, slot as u32)?;
    let item = items::key_of(&*lv.host, &held);
    let n = (held.count().min(255) as u8).min(amount);
    if n == 0 {
        return None;
    }
    if !simulate {
        let rest = held.shrink(u32::from(n));
        items::set_slot(lv, pos, slot as u32, rest);
    }
    Some((item, n))
}

/// Puts what fits of a stack into a block entity's slots `slots` (onto the same items, then
/// into empty slots: `insertItemStacked`); the count left.
fn put_slots(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    slots: std::ops::Range<usize>,
    item: &str,
    count: u8,
    simulate: bool,
) -> u8 {
    let max = stack_limit(&*lv.host, item);
    let mut current: Vec<create_engine::processing::Slot> =
        slots.clone().map(|i| slot_at(lv, pos, i)).collect();
    let before = current.clone();
    let left = create_engine::processing::insert_stacked(&mut current, item, count, max);
    if !simulate && left != count {
        let set: Vec<(u32, Option<mod_api::ItemStack>)> = slots
            .zip(current.into_iter().zip(before))
            .filter(|(_, (now, was))| now != was)
            .map(|(i, (s, _))| {
                let stack = s.and_then(|(id, n)| items::stack_of(&*lv.host, &id, u32::from(n)));
                (i as u32, stack)
            })
            .collect();
        items::set_slots(lv, pos, set);
    }
    left
}

/// An arm's points as the game holds them, for its tick.
struct ArmWorld<'a, 'l> {
    lv: &'a mut ServerLevel<'l>,
}

impl ArmWorld<'_, '_> {
    /// The first item on belt segment `index` (`getStackAtOffset`: past the segment's start, not
    /// past its end).
    fn belt_item(&self, c: [i32; 3], index: i32) -> Option<usize> {
        let dim = self.lv.dim;
        with_world(|w| {
            let b = w.dims[dim].belts.get(&c)?;
            b.items
                .items
                .iter()
                .position(|s| s.position > index as f32 && s.position <= index as f32 + 1.0)
        })
        .flatten()
    }
}

impl arm::World for ArmWorld<'_, '_> {
    fn deposited(&mut self, arm: [i32; 3], musical: bool) {
        crate::glue::advancements::arm_deposited(self.lv, arm, musical);
    }

    /// `isValid`: still a block of its type (`canCreatePoint`).
    fn valid(&mut self, p: &Point) -> bool {
        let Some(now) = arm_point(self.lv, p.pos) else {
            return false;
        };
        if now.kind != p.kind {
            return false;
        }
        match p.kind {
            // BeltType: a belt that carries items, with no tunnel over it.
            Kind::Belt => {
                let above = [p.pos[0], p.pos[1] + 1, p.pos[2]];
                let tunnel = matches!(
                    info_at(self.lv, above),
                    Some((Block::AndesiteTunnel | Block::BrassTunnel, _))
                );
                !tunnel && belt_segment(self.lv, p.pos).is_some_and(|(_, _, seg)| seg.transports())
            }
            // FunnelType: not extracting, not a belt funnel pushing onto its belt.
            Kind::Funnel => funnel_at(self.lv, p.pos).is_some_and(|f| {
                !f.extracting && f.belt != Some(create_engine::funnel::BeltShape::Pushing)
            }),
            _ => true,
        }
    }

    /// `checkForMusic`: a jukebox point with a record in it.
    fn playing(&mut self, p: &Point) -> bool {
        p.kind == Kind::Jukebox && property(self.lv, p.pos, "has_record").as_deref() == Some("true")
    }

    fn slots(&mut self, p: &Point) -> usize {
        match p.kind {
            // DepotItemHandler: the held stack and eight outputs.
            Kind::Depot => create_engine::depot::SLOTS,
            Kind::Belt | Kind::Chute | Kind::Deployer | Kind::Crafter => 1,
            // PackagerItemHandler: its box; the jukebox's record; the composter's bone meal
            // (Create's two slots, of which only the bottom's gives).
            Kind::Packager | Kind::Jukebox | Kind::Composter => 1,
            // The millstone's input and its nine outputs; the basin's nine and nine.
            Kind::Millstone => 10,
            Kind::Basin => create_engine::basin::SLOTS,
            _ => 0,
        }
    }

    fn extract(&mut self, p: &Point, slot: usize, amount: u8, simulate: bool) -> Option<Stack> {
        let lv = &mut *self.lv;
        let (dim, pos) = (lv.dim, p.pos);
        match p.kind {
            Kind::Depot => {
                let out = take_slot(lv, pos, slot, amount, simulate)?;
                if !simulate {
                    if slot == 0 && slot_at(lv, pos, 0).is_none() {
                        with_world(|w| w.depot_put(dim, pos, None));
                    } else {
                        with_world(|w| w.depot_changed(dim, pos));
                    }
                }
                Some(out)
            }
            Kind::Belt => {
                let (c, index, _) = belt_segment(lv, pos)?;
                let i = self.belt_item(c, index)?;
                with_world(|w| {
                    let d = &mut w.dims[dim];
                    let b = d.belts.get_mut(&c)?;
                    let s = b.items.items.get_mut(i)?;
                    let n = s.count.min(amount);
                    let out = (s.item.clone(), n);
                    if !simulate {
                        s.count -= n;
                        if s.count == 0 {
                            b.items.items.remove(i);
                        }
                        d.belt_changes.push(c);
                    }
                    Some(out)
                })
                .flatten()
            }
            Kind::Chute => {
                let s = chute_stack(lv, pos)?;
                let n = (s.count().min(255) as u8).min(amount);
                let out = (items::key_of(&*lv.host, &s), n);
                if !simulate {
                    set_chute_stack(lv, pos, s.shrink(u32::from(n)));
                    with_world(|w| w.dims[dim].chute_pulls.wake(pos));
                }
                Some(out)
            }
            // Only a millstone's outputs give (`MillstoneInventoryHandler`); a basin gives from
            // its inputs and outputs both (`CombinedInvWrapper` of two `BasinInventory`s).
            Kind::Millstone if slot >= 1 => take_slot(lv, pos, slot, amount, simulate),
            Kind::Basin if slot < create_engine::basin::SLOTS => {
                let out = take_slot(lv, pos, slot, amount, simulate)?;
                if !simulate {
                    with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
                }
                Some(out)
            }
            // CrafterPoint lets the arm take a crafter's item.
            Kind::Crafter => take_slot(lv, pos, 0, amount, simulate),
            Kind::Packager => {
                let b = crate::glue::packages::arm_extract(lv, pos, simulate)?;
                Some((items::key_of(&*lv.host, &b), b.count().min(255) as u8))
            }
            // JukeboxPoint.extract: the record, while it has one (`removeItem` stops it).
            Kind::Jukebox => {
                if property(lv, pos, "has_record").as_deref() != Some("true") {
                    return None;
                }
                let up = mod_api::Direction::Up;
                let record = peek(lv, pos, up)?;
                let out = (items::key_of(&*lv.host, &record), 1);
                if !simulate {
                    items::take_from(lv, pos, up, |_| true, 1, false)?;
                }
                Some(out)
            }
            // ComposterPoint.extract: what its bottom gives (the bone meal of a full one).
            Kind::Composter => {
                let side = mod_api::Direction::Down;
                let s = peek(lv, pos, side)?;
                let n = (s.count().min(255) as u8).min(amount);
                let out = (items::key_of(&*lv.host, &s), n);
                if !simulate {
                    items::take_from(lv, pos, side, |_| true, u32::from(n), false)?;
                }
                Some(out)
            }
            // DeployerItemHandler: its held stack.
            Kind::Deployer => {
                let mut d = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten()?;
                let (item, count) = d.held.clone()?;
                // An arm carries an item by its id: a stack with components stays in the hand.
                if !d.held_components.is_empty() {
                    return None;
                }
                // `DeployerItemHandler.extractItem`: what its filter names stays in its hand.
                let f = crate::glue::processing::machine_filter(dim, pos);
                if f.is_some()
                    && crate::glue::items::filters::passes_item(&*lv.host, f.as_ref(), &item)
                {
                    return None;
                }
                let n = count.min(amount);
                if !simulate {
                    d.held = (count > n).then(|| (item.clone(), count - n));
                    with_world(|w| w.set_deployer(dim, pos, d));
                }
                Some((item, n))
            }
            _ => None,
        }
    }

    fn insert(&mut self, p: &Point, item: &str, count: u8, simulate: bool) -> u8 {
        let lv = &mut *self.lv;
        let (dim, pos) = (lv.dim, p.pos);
        let max = stack_limit(&*lv.host, item);
        match p.kind {
            Kind::Depot => depot_insert(lv, pos, item, count, simulate),
            // ItemHandlerBeltSegment.insertItem: onto the segment from above while nothing just
            // put there is in the way.
            Kind::Belt => put_on_from_above(lv, pos, item, count, None, simulate).unwrap_or(count),
            // ChuteItemHandler: while it is empty and takes the item, a stack from the top.
            Kind::Chute => {
                let Some((block, powered)) = chute_at(lv, pos) else {
                    return count;
                };
                let Some(one) = items::stack_of(&*lv.host, item, 1) else {
                    return count;
                };
                if chute_stack(lv, pos).is_some() || !chute_accepts(lv, pos, block, powered, &one) {
                    return count;
                }
                let n = count.min(max);
                if !simulate {
                    set_chute_stack(lv, pos, items::stack_of(&*lv.host, item, u32::from(n)));
                    with_world(|w| w.dims[dim].chutes.insert(pos, 1.0));
                }
                count - n
            }
            // FunnelPoint: not while powered, only what its filter lets through; into the
            // inventory behind it.
            Kind::Funnel => {
                let Some(f) = funnel_at(lv, pos) else {
                    return count;
                };
                if f.powered {
                    return count;
                }
                let behind = create_engine::funnel::behind(pos, f.facing);
                let Some(stack) = items::stack_of(&*lv.host, item, u32::from(count)) else {
                    return count;
                };
                if !super::funnel_passes(lv, pos, &stack) {
                    return count;
                }
                if simulate {
                    let room = items::insert_room(lv, behind, face(f.facing), &stack).unwrap_or(0);
                    return count - room.min(u32::from(count)) as u8;
                }
                items::insert_item(lv, behind, face(f.facing), stack)
                    .map_or(0, |l| l.count().min(255) as u8)
            }
            // MillstoneInventoryHandler.isItemValid: only what it can mill (`canProcess`).
            Kind::Millstone => {
                let mills = crate::glue::processing::recipes::with(|r| {
                    r.milling.find_single(item).is_some()
                }) == Some(true);
                if !mills {
                    return count;
                }
                put_slots(lv, pos, 0..1, item, count, simulate)
            }
            // BlazeBurnerPoint: one at a time (`tryInsert` uses one up); a container left over
            // (a lava bucket's) comes back to the arm while it held only one, else drops.
            Kind::BlazeBurner => {
                let Some(one) = items::stack_of(&*lv.host, item, 1) else {
                    return count;
                };
                let Some(rest) =
                    crate::glue::processing::basin::feed_burner(lv, pos, &one, false, simulate)
                else {
                    return count;
                };
                if !simulate && let Some(r) = rest.filter(|_| count > 1) {
                    let at = pos.map(f64::from);
                    items::drop_stack(lv, at, [0.0; 3], r, 10);
                }
                count - 1
            }
            // CrushingWheelPoint: the controller's inventory takes a stack while empty.
            Kind::CrushingWheels => {
                let empty = with_world(|w| {
                    w.dims[dim]
                        .crushers
                        .get(&pos)
                        .is_some_and(|(c, _)| !c.occupied())
                })
                .unwrap_or(false);
                if !empty {
                    return count;
                }
                if !simulate {
                    let id = create_engine::arm::item_id(item);
                    let time = crate::glue::processing::recipes::with(|r| {
                        create_engine::crusher::recipe(&r.crushing, &r.milling, id).map(|x| x.time)
                    })
                    .flatten();
                    with_world(|w| {
                        if let Some((c, _)) = w.dims[dim].crushers.get_mut(&pos) {
                            create_engine::crusher::intake(c, id, count, time);
                        }
                        w.mark_unsaved(dim, pos);
                    });
                }
                0
            }
            // A saw facing up takes a stack while it holds nothing (`SawBlockEntity`'s
            // inventory, through `DepotPoint`).
            Kind::Saw => {
                let free = with_world(|w| {
                    w.dims[dim]
                        .saws
                        .get(&pos)
                        .is_none_or(|s| s.slots.is_empty())
                })
                .unwrap_or(false);
                if !free {
                    return count;
                }
                // One at a time (`ProcessingInventory`'s slot limit without bulk cutting).
                let id = create_engine::arm::item_id(item);
                let progress = items::stack_of(&*lv.host, item, 1)
                    .and_then(|s| crate::glue::processing::sequenced_progress(&s));
                if !simulate && !crate::glue::processing::saw::saw_put(lv, pos, id, progress) {
                    return count;
                }
                count - 1
            }
            // PackagerItemHandler.insertItem: one package, unwrapped.
            Kind::Packager => {
                let Some(one) = items::stack_of(&*lv.host, item, 1) else {
                    return count;
                };
                if crate::glue::packages::arm_insert(lv, pos, &one, simulate) {
                    count - 1
                } else {
                    count
                }
            }
            // CampfirePoint: one cookable item into a free spot (`placeFood`).
            Kind::Campfire => {
                let Some(one) = items::stack_of(&*lv.host, item, 1) else {
                    return count;
                };
                let kind = mod_api::Identifier::parse("minecraft:campfire_cooking").ok();
                let cookable = kind.is_some_and(|k| {
                    lv.level
                        .recipe_for(&*lv.host, &k, &mod_api::RecipeInput::Single(one.clone()))
                        .ok()
                        .flatten()
                        .is_some()
                });
                let held = items::entity(lv, pos)
                    .and_then(|e| e.data(&*lv.host))
                    .map_or(0, |d| crate::glue::packages::list(&d, "Items").len());
                if !cookable || held >= 4 {
                    return count;
                }
                if !simulate && use_on_top(lv, pos, one).is_some() {
                    return count;
                }
                count - 1
            }
            // ComposterPoint.insert: into its top.
            Kind::Composter => {
                let Some(stack) = items::stack_of(&*lv.host, item, u32::from(count)) else {
                    return count;
                };
                let up = mod_api::Direction::Up;
                if simulate {
                    let room = items::insert_room(lv, pos, up, &stack).unwrap_or(0);
                    return count - room.min(u32::from(count)) as u8;
                }
                items::insert_item(lv, pos, up, stack).map_or(0, |l| l.count().min(255) as u8)
            }
            // JukeboxPoint.insert: a record into an empty jukebox.
            Kind::Jukebox => {
                let Some(one) = items::stack_of(&*lv.host, item, 1) else {
                    return count;
                };
                let playable = lv
                    .host
                    .item_default_component(one.item(), "minecraft:jukebox_playable")
                    .is_some();
                if !playable || property(lv, pos, "has_record").as_deref() != Some("false") {
                    return count;
                }
                // Through the jukebox's item handler (`setTheItem`: it plays).
                let up = mod_api::Direction::Up;
                let went = if simulate {
                    items::insert_room(lv, pos, up, &one).unwrap_or(0)
                } else {
                    1 - items::insert_item(lv, pos, up, one).map_or(0, |l| l.count())
                };
                if went == 0 {
                    return count;
                }
                count - 1
            }
            // RespawnAnchorPoint: glowstone charges it, up to 4 (`RespawnAnchorBlock.charge`).
            Kind::RespawnAnchor => {
                let charge: u8 = property(lv, pos, "charge")
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(4);
                if create_engine::arm::item_id(item) != "minecraft:glowstone" || charge >= 4 {
                    return count;
                }
                if !simulate {
                    let next = (charge + 1).to_string();
                    lv.set_property(pos, "charge", &next, crate::glue::server::update_all());
                    let at = pos.map(|v| f64::from(v) + 0.5);
                    lv.level.play_sound(
                        &mut *lv.host,
                        "minecraft:block.respawn_anchor.charge",
                        at,
                        mod_api::SoundSource::Blocks,
                        1.0,
                        1.0,
                    );
                }
                count - 1
            }
            // BasinInventory.insertItem: one stack of a kind, in the slot that holds it, else in
            // the first empty one.
            Kind::Basin => {
                let inputs = create_engine::basin::INPUTS;
                let held: Vec<create_engine::processing::Slot> =
                    inputs.clone().map(|i| slot_at(lv, pos, i)).collect();
                let at = held
                    .iter()
                    .position(|s| s.as_ref().is_some_and(|(id, _)| id == item))
                    .or_else(|| held.iter().position(Option::is_none));
                let Some(at) = at else {
                    return count;
                };
                let slot = inputs.start + at;
                let left = put_slots(lv, pos, slot..slot + 1, item, count, simulate);
                if !simulate && left != count {
                    with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
                }
                left
            }
            // The crafter's handler (`ConnectedInputHandler`): one item into each empty, idle,
            // uncovered crafter of its group.
            Kind::Crafter => {
                let Some(stack) = items::stack_of(&*lv.host, item, u32::from(count)) else {
                    return count;
                };
                crate::glue::processing::crafter_handler_insert(lv, pos, stack, simulate)
                    .map_or(0, |l| l.count().min(255) as u8)
            }
            // DeployerItemHandler.insertItem: into an empty hand, or onto the same item.
            Kind::Deployer => {
                let Some(mut d) =
                    with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten()
                else {
                    return count;
                };
                // `isItemValid`: only what its filter lets through.
                let f = crate::glue::processing::machine_filter(dim, pos);
                if !crate::glue::items::filters::passes_item(
                    &*lv.host,
                    f.as_ref(),
                    create_engine::arm::item_id(item),
                ) {
                    return count;
                }
                let (now, left) = match d.held.clone() {
                    None => {
                        let n = count.min(max);
                        ((item.to_owned(), n), count - n)
                    }
                    Some((held, n)) if held == item => {
                        let room = max.saturating_sub(n);
                        if room == 0 {
                            return count;
                        }
                        let put = count.min(room);
                        ((held, n + put), count - put)
                    }
                    Some(_) => return count,
                };
                if !simulate {
                    d.held = Some(now);
                    with_world(|w| w.set_deployer(dim, pos, d));
                }
                left
            }
        }
    }

    /// BeltPoint.keepAlive: the first item on the segment stays still this tick.
    fn keep_alive(&mut self, p: &Point) {
        if p.kind != Kind::Belt {
            return;
        }
        let Some((c, index, _)) = belt_segment(self.lv, p.pos) else {
            return;
        };
        let dim = self.lv.dim;
        let centre = index as f32 + 0.5;
        with_world(|w| {
            if let Some(b) = w.dims[dim].belts.get_mut(&c)
                && let Some(s) = b
                    .items
                    .items
                    .iter_mut()
                    .find(|s| (s.position - centre).abs() < 0.51)
            {
                s.held_externally = true;
            }
        });
    }
}

/// `DepotItemHandler.insertItem` (slot 0) for the depot or ejector at `pos`: a depot takes a
/// stack while it holds nothing (`canMergeItems` false); an ejector only while charged, and
/// merges onto what it holds up to its stack size (`enableMerging`, `getRemainingSpace`).
pub(super) fn depot_insert(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    item: &str,
    count: u8,
    simulate: bool,
) -> u8 {
    let dim = lv.dim;
    let stacks = depot_items(lv, pos);
    let held = stacks.iter().find(|s| s.0 == 0).cloned();
    let max = stack_limit(&*lv.host, item);
    let ejector = with_world(|w| w.dims[dim].ejectors.get(&pos).cloned()).flatten();
    let room = match &ejector {
        None => {
            if !stacks.is_empty() {
                return count;
            }
            max
        }
        Some(e) => {
            if e.state != ejector::State::Charged || held.as_ref().is_some_and(|h| h.1 != item) {
                return count;
            }
            let limit = if e.max_stack == 0 { 64 } else { e.max_stack }.min(max);
            let present: u32 = stacks.iter().map(|s| u32::from(s.2)).sum();
            u32::from(limit).saturating_sub(present).min(255) as u8
        }
    };
    let n = count.min(room);
    if n == 0 {
        return count;
    }
    if !simulate {
        let had = held.as_ref().map_or(0, |h| h.2);
        let stack = items::stack_of(&*lv.host, item, u32::from(had + n));
        if !items::set_slot(lv, pos, 0, stack) {
            return count;
        }
        if held.is_none() {
            let h =
                create_engine::depot::Held::new(random_angle(lv, pos), upright(&*lv.host, item));
            with_world(|w| w.depot_put(dim, pos, Some(h)));
        } else {
            with_world(|w| w.depot_changed(dim, pos));
        }
    }
    count - n
}

/// Arms and ejectors whose redstone input may have changed read it (`redstoneUpdate`: a newly
/// freed arm looks for items at once; `updateSignal`).
fn tick_arm_signals(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let checks = with_world(|w| std::mem::take(&mut w.dims[dim].arm_signals)).unwrap_or_default();
    for pos in checks {
        let powered = lv.has_neighbor_signal(pos);
        let arm = with_world(|w| w.dims[dim].arms.arms.remove(&pos)).flatten();
        if let Some(mut a) = arm {
            let changed = a.set_powered(pos, powered, &mut ArmWorld { lv: &mut *lv });
            with_world(|w| {
                let arms = &mut w.dims[dim].arms;
                arms.arms.insert(pos, a);
                arms.wake(pos);
                if changed {
                    arms.changes.push(pos);
                }
            });
            continue;
        }
        with_world(|w| {
            let d = &mut w.dims[dim];
            if let Some(e) = d.ejectors.get_mut(&pos)
                && e.powered != powered
            {
                e.powered = powered;
                if !d.ejecting.contains(&pos) {
                    d.ejecting.push(pos);
                }
            }
        });
    }
}

/// Mechanical arms: their redstone, the lazy tick of sleeping arms that poll, then the awake
/// ones (`ArmBlockEntity.tick`); what removed arms held drops; clients see the arms whose target
/// or phase changed. Nothing runs without an awake arm.
fn tick_arms(lv: &mut ServerLevel<'_>) {
    tick_arm_signals(lv);
    let dim = lv.dim;
    let now = lv.game_time().max(0) as u64;
    let running = with_world(|w| {
        let arms = &mut w.dims[dim].arms;
        arms.poll(now);
        !arms.running.is_empty()
    })
    .unwrap_or(false);
    if running {
        // The arms leave the world while they tick: their points reach into it.
        let mut arms = with_world(|w| std::mem::take(&mut w.dims[dim].arms)).unwrap_or_default();
        let speeds: HashMap<[i32; 3], f32> = arms
            .running
            .iter()
            .map(|p| {
                (
                    *p,
                    with_world(|w| w.speed(dim, *p)).flatten().unwrap_or(0.0),
                )
            })
            .collect();
        arms.tick(
            |p| speeds.get(&p).copied().unwrap_or(0.0),
            &mut ArmWorld { lv: &mut *lv },
        );
        with_world(|w| {
            let back = std::mem::replace(&mut w.dims[dim].arms, arms);
            // Changes reported while they ticked.
            let a = &mut w.dims[dim].arms;
            a.changes.extend(back.changes);
            for (p, arm) in back.arms {
                a.arms.entry(p).or_insert(arm);
            }
        });
    }
    let (drops, changes) = with_world(|w| {
        let d = &mut w.dims[dim];
        let mut changes = std::mem::take(&mut d.arms.changes);
        changes.sort();
        changes.dedup();
        for c in &changes {
            if d.arms.arms.contains_key(c) {
                d.unsaved.push((c[0] >> 4, c[2] >> 4));
            }
        }
        (std::mem::take(&mut d.arm_drops), changes)
    })
    .unwrap_or_default();
    for (pos, item, n) in drops {
        let at = pos.map(|v| f64::from(v) + 0.5);
        items::drop_item(lv, at, [0.0; 3], &item, u32::from(n), 0);
    }
    if !changes.is_empty() {
        let players = lv.players();
        send_arms(lv, &players, &changes);
    }
}

/// What changes at a point wakes the arms using it: depots (and ejectors), belts (any of their
/// segments) and chutes report it.
pub fn arms_touched(dim: usize, depots: &[[i32; 3]], belts: &[[i32; 3]]) {
    with_world(|w| {
        if w.dims[dim].arms.watch.is_empty() {
            return;
        }
        let d = &mut w.dims[dim];
        for p in depots {
            d.arms.touched(*p);
        }
        for c in belts {
            if let Some(b) = d.belts.get(c) {
                for p in b.chain.clone() {
                    d.arms.touched(p);
                }
            }
        }
    });
}

/// An arm or ejector whose speed changed ticks again.
fn wake_on_speed(dim: usize, pos: [i32; 3]) {
    with_world(|w| {
        let d = &mut w.dims[dim];
        if d.arms.arms.contains_key(&pos) {
            d.arms.wake(pos);
            d.arms.changes.push(pos);
        }
        if d.ejectors.contains_key(&pos) {
            if !d.ejecting.contains(&pos) {
                d.ejecting.push(pos);
            }
            d.ejector_changes.push(pos);
        }
    });
}

/// Arms to `players`.
fn send_arms(lv: &mut ServerLevel<'_>, players: &[PlayerRef], list: &[[i32; 3]]) {
    let dim = lv.dim;
    let shown: Vec<arm::Shown> = list
        .iter()
        .map(|&pos| {
            with_world(|w| {
                w.dims[dim]
                    .arms
                    .arms
                    .get(&pos)
                    .map(|a| arm::Shown::of(pos, a))
            })
            .flatten()
            .unwrap_or(arm::Shown {
                pos,
                gone: true,
                ceiling: false,
                goggles: false,
                phase: arm::Phase::SearchInputs,
                progress: 1.0,
                from: None,
                to: None,
                from_base: 0.0,
                held: None,
                points: Vec::new(),
            })
        })
        .collect();
    for m in arm::encode(&shown) {
        net::send_to_players(&mut *lv.host, players, arm::CHANNEL, &m);
    }
}

/// `ArmBlock.useItemOn` with something held: it goes to the player, the arm looks for inputs.
/// True when the click was the arm's.
pub fn use_arm(lv: &mut ServerLevel<'_>, pos: [i32; 3], player: u128) -> bool {
    let dim = lv.dim;
    let held = with_world(|w| {
        let arms = &mut w.dims[dim].arms;
        let held = arms.arms.get_mut(&pos)?.take_held()?;
        arms.wake(pos);
        arms.changes.push(pos);
        Some(held)
    })
    .flatten();
    let Some((item, n)) = held else {
        return false;
    };
    if let Some(stack) = items::stack_of(&*lv.host, &item, u32::from(n)) {
        items::give(&mut *lv.host, player, stack);
    }
    true
}

/// A weighted ejector's surroundings as its tick reaches them.
struct EjectorWorld<'a, 'l> {
    lv: &'a mut ServerLevel<'l>,
    pos: [i32; 3],
}

impl EjectorWorld<'_, '_> {
    /// The funnel on top of the ejector facing its back (`getFunnelFacing(above) ==
    /// getFacing().getOpposite()`), if there is one.
    fn funnel_on_top(&self) -> Option<([i32; 3], super::funnel::FunnelState)> {
        let facing = info_at(self.lv, self.pos)?.1.facing?;
        let above = [self.pos[0], self.pos[1] + 1, self.pos[2]];
        let f = funnel_at(self.lv, above)?;
        (f.facing == facing.opposite()).then_some((above, f))
    }
}

/// `DirectBeltInputBehaviour.tryExportingToBeltFunnel(stack, null, simulate)` of a depot or
/// ejector under the funnel `f` at `pos`: None unless it is a pulling belt funnel; a powered one
/// takes nothing; else what is left of `stack` once it took what its inventory takes and its
/// filter passes (`FunnelBlock.tryInsert`).
pub(super) fn export_to_belt_funnel(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    f: super::funnel::FunnelState,
    stack: &mod_api::ItemStack,
    simulate: bool,
) -> Option<u32> {
    if f.belt != Some(create_engine::funnel::BeltShape::Pulling) {
        return None;
    }
    let count = stack.count();
    if f.powered || !super::funnel::funnel_passes(lv, pos, stack) {
        return Some(count);
    }
    let behind = create_engine::funnel::behind(pos, f.facing);
    let side = items::face(f.facing);
    if simulate {
        let room = items::insert_room(lv, behind, side, stack).unwrap_or(0);
        return Some(count.saturating_sub(room));
    }
    let left = items::insert_item(lv, behind, side, stack.clone()).map_or(0, |l| l.count());
    if left != count {
        super::flaps::funnel(lv, pos, true);
        crate::glue::redstone::detectors::funnel_transfer(lv, pos, stack);
    }
    Some(left)
}

/// `DirectBeltInputBehaviour.handleInsertion(stack, UP)` at `target`: a depot or ejector (empty,
/// charged), else a belt segment that carries items. The count left, None when nothing there
/// takes items.
fn land(
    lv: &mut ServerLevel<'_>,
    target: [i32; 3],
    item: &str,
    count: u8,
    simulate: bool,
) -> Option<u8> {
    let dim = lv.dim;
    // An ejector merges what lands onto what it holds (`enableMerging`).
    if is_ejector(dim, target) {
        return Some(depot_insert(lv, target, item, count, simulate));
    }
    let is_depot = with_world(|w| w.dims[dim].depots.contains_key(&target)) == Some(true);
    if is_depot {
        if !depot_items(lv, target).is_empty() || !ejector_accepts(dim, target) {
            return Some(count);
        }
        let n = count.min(stack_limit(&*lv.host, item));
        if !simulate {
            let stack = items::stack_of(&*lv.host, item, u32::from(n));
            if !items::set_slot(lv, target, 0, stack) {
                return None;
            }
            let h =
                create_engine::depot::Held::new(random_angle(lv, target), upright(&*lv.host, item));
            with_world(|w| w.depot_put(dim, target, Some(h)));
        }
        return Some(count - n);
    }
    // Funnels, chutes, item drains and processing machines: their own
    // `handleInsertion(stack, UP, simulate)`.
    let belt = with_world(|w| w.dims[dim].belt_of.contains_key(&target)) == Some(true);
    if !belt && super::belt::direct_input_at(lv, target) {
        let one = create_engine::belt::Item::new(item, count, random_angle(lv, target), false);
        return super::belt::handle_insertion(lv, target, Dir::Up, &one, false, simulate);
    }
    put_on_from_above(lv, target, item, count, None, simulate)
}

impl ejector::World for EjectorWorld<'_, '_> {
    fn present(&mut self) -> ejector::Present {
        let dim = self.lv.dim;
        let stacks = depot_items(self.lv, self.pos);
        let held = stacks.iter().any(|s| s.0 == 0);
        let position = with_world(|w| w.dims[dim].depots.get(&self.pos).copied().flatten())
            .flatten()
            .map_or(0.5, |h| h.position);
        ejector::Present {
            count: stacks.iter().map(|s| u32::from(s.2)).sum(),
            held_position: held.then_some(position),
        }
    }

    fn target_refuses(&mut self, target: [i32; 3]) -> bool {
        let Some((_, item, count)) = depot_items(self.lv, self.pos)
            .into_iter()
            .find(|s| s.0 == 0)
        else {
            return false;
        };
        land(self.lv, target, &item, count, true) == Some(count)
    }

    fn funnel_on_top_takes(&mut self) -> Option<bool> {
        let (above, f) = self.funnel_on_top()?;
        let held = items::slot(self.lv, self.pos, 0)?;
        Some(export_to_belt_funnel(self.lv, above, f, &held, true) == Some(0))
    }

    fn take_all(&mut self) -> Vec<(String, u8)> {
        // `launchItems`: a pulling belt funnel on top facing its back takes what it can first.
        if let Some((above, f)) = self.funnel_on_top() {
            for slot in 0..create_engine::depot::SLOTS as u32 {
                let Some(stack) = items::slot(self.lv, self.pos, slot) else {
                    continue;
                };
                let count = stack.count();
                let Some(left) = export_to_belt_funnel(self.lv, above, f, &stack, false) else {
                    continue;
                };
                if left != count {
                    let rest = items::with_count(&stack, left).filter(|_| left > 0);
                    items::set_slot(self.lv, self.pos, slot, rest);
                }
            }
        }
        let Some(slots) = items::slots(self.lv, self.pos, create_engine::depot::SLOTS as u32)
        else {
            return Vec::new();
        };
        let taken: Vec<(String, u8)> = slots
            .iter()
            .flatten()
            .map(|s| (items::key_of(&*self.lv.host, s), s.count().min(255) as u8))
            .collect();
        if !taken.is_empty() {
            let clear = (0..create_engine::depot::SLOTS as u32).map(|i| (i, None));
            items::set_slots(self.lv, self.pos, clear);
            let dim = self.lv.dim;
            with_world(|w| w.depot_put(dim, self.pos, None));
        }
        taken
    }

    fn insert(&mut self, target: [i32; 3], item: &str, count: u8) -> Option<u8> {
        land(self.lv, target, item, count, false)
    }

    fn drop_item(&mut self, at: [f64; 3], motion: [f64; 3], item: &str, count: u8) {
        items::drop_item(self.lv, at, motion, item, u32::from(count), 10);
    }

    fn clip(&mut self, from: [f64; 3], to: [f64; 3]) -> Option<([f64; 3], [i32; 3], Dir)> {
        let own = self.pos;
        let lv = &*self.lv;
        ejector::clip_blocks(from, to, |p| p != own && has_collision(lv, p))
    }

    fn extracting_funnel(&mut self, pos: [i32; 3]) -> bool {
        funnel_at(self.lv, pos).is_some_and(|f| f.belt.is_none() && f.extracting)
    }
}

/// Weighted ejectors that have something to do (`EjectorBlockEntity.tick`): winding up,
/// launching, stacks in flight, or charged with something on them; the rest cost nothing.
/// Stacks of removed ejectors drop; clients see the ejectors whose plate or flight changed.
fn tick_ejectors(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let list = with_world(|w| w.dims[dim].ejecting.clone()).unwrap_or_default();
    for pos in list {
        let Some(facing) = info_at(lv, pos).and_then(|(_, i)| i.facing) else {
            continue;
        };
        let Some(mut e) = with_world(|w| w.dims[dim].ejectors.remove(&pos)).flatten() else {
            continue;
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let flying = e.flying.len();
        let before = e.state;
        let changed = e.tick(pos, facing, speed, &mut EjectorWorld { lv: &mut *lv, pos });
        if before == ejector::State::Charged && e.state == ejector::State::Launching {
            launch_entities(lv, pos, e.launcher.entity_motion(facing));
        }
        let has_items = !depot_items(lv, pos).is_empty();
        let busy = e.busy(speed, has_items);
        with_world(|w| {
            let d = &mut w.dims[dim];
            d.ejectors.insert(pos, e);
            if changed {
                d.ejector_changes.push(pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            } else if flying > 0 {
                // Stacks still in flight are saved with it.
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if !busy {
                d.ejecting.retain(|p| *p != pos);
            }
        });
    }
    let (drops, changes) = with_world(|w| {
        let d = &mut w.dims[dim];
        let mut c = std::mem::take(&mut d.ejector_changes);
        c.sort();
        c.dedup();
        (std::mem::take(&mut d.ejector_drops), c)
    })
    .unwrap_or_default();
    for (at, v, item, n) in drops {
        items::drop_item(lv, at, v, &item, u32::from(n), 0);
    }
    if !changes.is_empty() {
        let players = lv.players();
        send_ejectors(lv, &players, &changes);
    }
}

thread_local! {
    /// Entities that stood on an ejector this tick (`block_step_on`): level, ejector, entity.
    static STEPPED: std::cell::RefCell<Vec<(usize, [i32; 3], u32)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// `Block.stepOn` of an ejector by something but an item or a player (whose own client asks,
/// `trigger_ejector`): noted for `entities_on_ejectors`.
pub fn ejector_stepped_on(dim: usize, pos: [i32; 3], entity: u32) {
    STEPPED.with_borrow_mut(|s| s.push((dim, pos, entity)));
}

/// `EjectorBlock.updateEntityAfterFallOn` for what stood on an ejector this tick: a charged,
/// unpowered ejector with a target ahead fires; an entity more than 4 px from the plate's
/// middle is first drawn halfway to it (moving down at 0.125) and fires it once there.
fn entities_on_ejectors(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut stepped: Vec<([i32; 3], u32)> = STEPPED.with_borrow_mut(|s| {
        let mine = s
            .iter()
            .filter(|e| e.0 == dim)
            .map(|e| (e.1, e.2))
            .collect();
        s.retain(|e| e.0 != dim);
        mine
    });
    stepped.sort();
    stepped.dedup();
    for (pos, id) in stepped {
        let ready = with_world(|w| {
            w.dims[dim].ejectors.get(&pos).is_some_and(|e| {
                e.state != ejector::State::Retracting && !e.powered && e.launcher.h != 0
            })
        })
        .unwrap_or(false);
        let Some(e) = lv.host.entity(id) else {
            continue;
        };
        if !ready {
            continue;
        }
        if e.on_ground(&*lv.host) == Some(true) {
            let Some(at) = e.position(&*lv.host) else {
                continue;
            };
            let centre = [
                f64::from(pos[0]) + 0.5,
                f64::from(pos[1]) + 0.5 + 7.0 / 16.0,
                f64::from(pos[2]) + 0.5,
            ];
            let diff = (0..3)
                .map(|a| (centre[a] - at[a]).powi(2))
                .sum::<f64>()
                .sqrt();
            e.set_velocity(&mut *lv.host, [0.0, -0.125, 0.0]);
            if diff > 4.0 / 16.0 {
                let mid = [0, 1, 2].map(|a| (centre[a] + at[a]) * 0.5);
                e.teleport_to(&mut *lv.host, mid, None);
                continue;
            }
        }
        trigger_ejector(dim, pos);
    }
}

/// `EjectorBlockEntity.activate` (`EjectorTriggerPacket` from a player's client, or something
/// landing on it).
pub fn trigger_ejector(dim: usize, pos: [i32; 3]) {
    with_world(|w| {
        let d = &mut w.dims[dim];
        let Some(e) = d.ejectors.get_mut(&pos) else {
            return;
        };
        if e.state == ejector::State::Retracting || e.powered || e.launcher.h == 0 {
            return;
        }
        e.activate();
        if !d.ejecting.contains(&pos) {
            d.ejecting.push(pos);
        }
    });
}

/// `activateDeferred`'s entities: what stands on the plate but items and players (whose own
/// clients throw them) is thrown from its middle (`setPos`, `applyMotion`).
fn launch_entities(lv: &mut ServerLevel<'_>, pos: [i32; 3], motion: [f64; 3]) {
    let inset = 1.0 / 16.0;
    let lo = [
        f64::from(pos[0]) + inset,
        f64::from(pos[1]),
        f64::from(pos[2]) + inset,
    ];
    let hi = [
        f64::from(pos[0] + 1) - inset,
        f64::from(pos[1] + 1),
        f64::from(pos[2] + 1) - inset,
    ];
    let found = lv.level.entities_in(&*lv.host, lo, hi).unwrap_or_default();
    for e in found {
        let kind = e
            .kind(&*lv.host)
            .map(|k| k.id(&*lv.host))
            .unwrap_or_default();
        if matches!(
            kind.as_str(),
            "minecraft:item" | "minecraft:player" | "minecraft:experience_orb"
        ) {
            continue;
        }
        let at = [
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 1.0,
            f64::from(pos[2]) + 0.5,
        ];
        e.teleport_to(&mut *lv.host, at, None);
        e.set_velocity(&mut *lv.host, motion);
    }
}

/// An ejector starts ticking: its depot changed, it started turning.
pub fn wake_ejectors(dim: usize, list: &[[i32; 3]]) {
    with_world(|w| {
        let d = &mut w.dims[dim];
        if d.ejectors.is_empty() {
            return;
        }
        for p in list {
            if d.ejectors.contains_key(p) && !d.ejecting.contains(p) {
                d.ejecting.push(*p);
            }
        }
    });
}

/// Ejectors to `players`.
fn send_ejectors(lv: &mut ServerLevel<'_>, players: &[PlayerRef], list: &[[i32; 3]]) {
    let dim = lv.dim;
    let shown: Vec<ejector::Shown> = list
        .iter()
        .map(|&pos| {
            let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
            with_world(|w| {
                w.dims[dim]
                    .ejectors
                    .get(&pos)
                    .map(|e| ejector::Shown::of(pos, e, speed))
            })
            .flatten()
            .unwrap_or(ejector::Shown {
                pos,
                gone: true,
                state: ejector::State::Charged,
                lid: 0.0,
                wind_up: 0.0,
                h: 1,
                v: 0,
                early: None,
                flying: Vec::new(),
            })
        })
        .collect();
    for m in ejector::encode(&shown) {
        net::send_to_players(&mut *lv.host, players, ejector::CHANNEL, &m);
    }
}

/// A joining player sees every arm and ejector of its level.
fn sync_arms(lv: &mut ServerLevel<'_>, player: PlayerRef) {
    let dim = lv.dim;
    let arms: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].arms.arms.keys().copied().collect()).unwrap_or_default();
    send_arms(lv, &[player], &arms);
    let ejectors: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].ejectors.keys().copied().collect()).unwrap_or_default();
    send_ejectors(lv, &[player], &ejectors);
}

/// An arm as `arms_to_nbt` saves it: x, y, z, its phase, index, progress, power, selection,
/// held count, input count, then each point's offset from it, kind and facing; its held item
/// and goggles.
fn arm_to_tag(p: [i32; 3], a: &arm::Arm) -> Compound {
    let mut v = vec![
        p[0],
        p[1],
        p[2],
        i32::from(a.phase.index()),
        a.index,
        a.progress.to_bits() as i32,
        i32::from(a.powered),
        a.selection as i32,
        i32::from(a.held.as_ref().map_or(0, |h| h.1)),
        a.inputs.len() as i32,
    ];
    for pt in a.points() {
        v.extend([
            pt.pos[0] - p[0],
            pt.pos[1] - p[1],
            pt.pos[2] - p[2],
            i32::from(pt.kind.index()),
            pt.facing.map_or(-1, |f| f as i32),
        ]);
    }
    let mut c = Compound::new();
    c.insert("v".into(), Tag::IntArray(v));
    let held = a.held.as_ref().map_or("", |h| h.0.as_str());
    c.insert("held".into(), Tag::String(held.into()));
    if a.goggles {
        c.insert("Goggles".into(), Tag::Byte(1));
    }
    c
}

/// Arms and ejectors saved with their column (`ArmBlockEntity.write`: `InteractionPoints`,
/// `Phase`, `Powered`, `HeldItem`, `TargetPointIndex`, `MovementProgress`, the selection mode;
/// `EjectorBlockEntity.write`: `HorizontalDistance`, `VerticalDistance`, `Powered`, `State`,
/// `Lid`, `LaunchedItems`, the stack size): `arms` and `ejectors`, absent when the column has
/// none.
fn arms_to_nbt(dim: usize, chunk: [i32; 2], own: &mut Compound) {
    let arms = with_world(|w| w.dims[dim].arms.column(chunk[0], chunk[1])).unwrap_or_default();
    let mut ejectors: Vec<([i32; 3], ejector::Ejector)> = with_world(|w| {
        w.dims[dim]
            .ejectors
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]))
            .map(|(p, e)| (*p, e.clone()))
            .collect()
    })
    .unwrap_or_default();
    ejectors.sort_by_key(|e| e.0);
    if !arms.is_empty() {
        let list = arms
            .iter()
            .map(|(p, a)| Tag::Compound(arm_to_tag(*p, a)))
            .collect();
        own.insert("arms".into(), Tag::List(list));
    }
    if !ejectors.is_empty() {
        let list = ejectors
            .iter()
            .map(|(p, e)| {
                let mut v = vec![
                    p[0],
                    p[1],
                    p[2],
                    e.launcher.h,
                    e.launcher.v,
                    i32::from(e.state.index()),
                    e.lid.value.to_bits() as i32,
                    i32::from(e.powered),
                    i32::from(e.max_stack),
                ];
                for f in &e.flying {
                    v.extend([f.time, i32::from(f.count)]);
                }
                let mut c = Compound::new();
                c.insert("v".into(), Tag::IntArray(v));
                let items = e
                    .flying
                    .iter()
                    .map(|f| Tag::String(f.item.as_str().into()))
                    .collect();
                c.insert("flying".into(), Tag::List(items));
                Tag::Compound(c)
            })
            .collect();
        own.insert("ejectors".into(), Tag::List(list));
    }
}

/// What [`arm_to_tag`] wrote: the arm's position and the arm.
pub(super) fn arm_from_tag(c: &Compound) -> Option<([i32; 3], arm::Arm)> {
    let text = |t: Option<&Tag>| match t {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let Some(Tag::IntArray(v)) = c.get(&"v".into()) else {
        return None;
    };
    if v.len() < 10 {
        return None;
    }
    let pos = [v[0], v[1], v[2]];
    let mut a = arm::Arm::new(false);
    a.phase = arm::Phase::from_index(v[3].clamp(0, 4) as u8);
    a.goggles = matches!(c.get(&"Goggles".into()), Some(Tag::Byte(1)));
    a.index = v[4];
    a.progress = f32::from_bits(v[5] as u32).clamp(0.0, 1.0);
    a.powered = v[6] != 0;
    a.selection = match v[7] {
        1 => arm::Selection::ForcedRoundRobin,
        2 => arm::Selection::PreferFirst,
        _ => arm::Selection::RoundRobin,
    };
    let held = text(c.get(&"held".into()));
    let n = v[8].clamp(0, 64) as u8;
    a.held = (!held.is_empty() && n > 0).then_some((held, n));
    let inputs = v[9].max(0) as usize;
    for (i, pt) in v[10..].as_chunks::<5>().0.iter().enumerate() {
        let Some(kind) = Kind::from_index(pt[3].clamp(0, 255) as u8) else {
            continue;
        };
        let point = Point {
            pos: [pos[0] + pt[0], pos[1] + pt[1], pos[2] + pt[2]],
            kind,
            facing: usize::try_from(pt[4])
                .ok()
                .and_then(|i| Dir::ALL.get(i))
                .copied(),
        };
        if i < inputs {
            a.inputs.push(point);
        } else {
            a.outputs.push(point);
        }
    }
    Some((pos, a))
}

/// What `arms_to_nbt` wrote: arms and ejectors waiting for their blocks.
fn arms_from_nbt(dim: usize, data: &Compound) {
    let text = |t: Option<&Tag>| match t {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let mut arms = Vec::new();
    if let Some(Tag::List(list)) = data.get(&"arms".into()) {
        for t in list {
            let Tag::Compound(c) = t else { continue };
            arms.extend(arm_from_tag(c));
        }
    }
    let mut ejectors = Vec::new();
    if let Some(Tag::List(list)) = data.get(&"ejectors".into()) {
        for t in list {
            let Tag::Compound(c) = t else { continue };
            let Some(Tag::IntArray(v)) = c.get(&"v".into()) else {
                continue;
            };
            if v.len() < 9 {
                continue;
            }
            let pos = [v[0], v[1], v[2]];
            let mut e = ejector::Ejector {
                launcher: ejector::Launcher::new(v[3], v[4]),
                ..Default::default()
            };
            e.launcher.clamp(ejector::MAX_DISTANCE);
            e.state = ejector::State::from_index(v[5].clamp(0, 2) as u8);
            let lid = f32::from_bits(v[6] as u32).clamp(0.0, 1.0);
            e.lid.value = lid;
            e.lid.previous = lid;
            e.lid.target = lid;
            e.powered = v[7] != 0;
            e.max_stack = v[8].clamp(0, 64) as u8;
            let items: Vec<String> = match c.get(&"flying".into()) {
                Some(Tag::List(l)) => l.iter().map(|t| text(Some(t))).collect(),
                _ => Vec::new(),
            };
            for (f, item) in v[9..].as_chunks::<2>().0.iter().zip(items) {
                let id = e.next_id;
                e.next_id += 1;
                e.flying.push(ejector::Flying {
                    id,
                    time: f[0],
                    item,
                    count: f[1].clamp(1, 64) as u8,
                });
            }
            ejectors.push((pos, e));
        }
    }
    if arms.is_empty() && ejectors.is_empty() {
        return;
    }
    with_world(|w| {
        let d = &mut w.dims[dim];
        d.arms.saved.extend(arms);
        d.ejector_saved.extend(ejectors);
    });
}

/// `ArmBlockEntity.getRange()`: Create's default `mechanicalArmRange`.
const ARM_RANGE: i32 = 5;

thread_local! {
    /// The points each player chose with the arm item in hand (`ArmInteractionPointHandler`'s
    /// `currentSelection`, kept on the server here): each with whether it is an input (TAKE).
    static SELECTION: std::cell::RefCell<HashMap<u128, Vec<(Point, bool)>>> =
        std::cell::RefCell::default();
}

/// Whether `stack` is a mechanical arm.
fn is_arm_item(host: &dyn mod_api::Host, stack: Option<&mod_api::ItemStack>) -> bool {
    stack.is_some_and(|s| items::id_of(host, s) == "create:mechanical_arm")
}

/// `ArmInteractionPointHandler.rightClickingBlocksSelectsThem`: with the arm in the main hand,
/// a click on a block an arm can work with selects it, first as an input (a funnel, only ever
/// an output), and each click after swaps its mode; the action bar says which. The click is the
/// selection's, so no arm is placed. On a client it only keeps the arm from being placed there.
pub fn select_point(
    host: mod_api::HostSide,
    event: &mut mod_api::RightClickBlockEvent<'_>,
) -> mod_api::Flow {
    let click = event.click;
    let selected = match host {
        mod_api::HostSide::Client(host) => {
            let state = host
                .level(host.dimension())
                .and_then(|l| l.block_state(&*host, click.pos));
            let kind = state.and_then(|state| match super::super::states::id(state) {
                Some(id) => super::super::client_table(&*host, |t| t.get(id).map(|(b, _)| b))
                    .flatten()
                    .and_then(point_kind),
                None => {
                    let text = host.block_state_string(state)?;
                    vanilla_point_kind(super::super::states::split(&text).0)
                }
            });
            click.hand == mod_api::Hand::Main && is_arm_item(&*host, click.stack) && kind.is_some()
        }
        mod_api::HostSide::Server(host) => select_on_server(host, click),
    };
    if !selected {
        return mod_api::Flow::Continue;
    }
    *event.cancellation_result = mod_api::InteractionResult::Success;
    mod_api::Flow::Cancel
}

/// `ArmBlock.useItemOn` with goggles: an arm without them puts them on (the goggles stay in
/// hand). Whether the click was taken.
fn put_on_goggles(
    host: &mut mod_api::HostSide,
    event: &mut mod_api::RightClickBlockEvent<'_>,
) -> bool {
    let click = event.click;
    if click.hand != mod_api::Hand::Main {
        return false;
    }
    let taken = match host {
        mod_api::HostSide::Client(h) => {
            let goggles = click
                .stack
                .is_some_and(|s| s.item().id(&**h) == "create:goggles");
            let arm = h
                .level(h.dimension())
                .and_then(|l| l.block_state(&**h, click.pos))
                .and_then(super::super::states::id)
                .and_then(|id| {
                    super::super::client_table(&**h, |t| t.get(id).map(|(b, _)| b)).flatten()
                })
                == Some(Block::MechanicalArm);
            goggles && arm && !super::client::arm_goggles(click.pos)
        }
        mod_api::HostSide::Server(h) => {
            if !click
                .stack
                .is_some_and(|s| s.item().id(&**h) == "create:goggles")
            {
                return false;
            }
            let dimension = click.dimension.key().id().to_string();
            let Some(mut lv) = ServerLevel::new(&mut **h, &dimension) else {
                return false;
            };
            let dim = lv.dim;
            let pos = click.pos;
            let put = with_world(|w| {
                let a = w.dims[dim].arms.arms.get_mut(&pos)?;
                if a.goggles {
                    return None;
                }
                a.goggles = true;
                w.mark_unsaved(dim, pos);
                Some(())
            })
            .flatten()
            .is_some();
            if put {
                let players = lv.players();
                send_arms(&mut lv, &players, &[pos]);
            }
            put
        }
    };
    if taken {
        *event.cancellation_result = mod_api::InteractionResult::Success;
    }
    taken
}

/// `select_point` on the server: true when the click selected a point.
fn select_on_server(host: &mut dyn ServerHost, click: &mod_api::BlockClick<'_>) -> bool {
    if click.hand != mod_api::Hand::Main || !is_arm_item(&*host, click.stack) {
        return false;
    }
    let dimension = click.dimension.key().id().to_owned();
    let Some(lv) = ServerLevel::new(host, &dimension) else {
        return false;
    };
    let Some(player) = lv.host.player(click.player) else {
        return false;
    };
    if player.entity().spectator(&*lv.host) == Some(true) {
        return false;
    }
    let Some(point) = arm_point(&lv, click.pos) else {
        return false;
    };
    // `state.getBlock().getName()`.
    let name_key = match lv.block_name(click.pos) {
        Some(name) => {
            let (ns, path) = name.split_once(':').unwrap_or(("minecraft", &name));
            format!("block.{ns}.{path}")
        }
        None => return false,
    };
    // `cycleMode`: a new point starts as an output and turns at once; a deposit-only point
    // stays an output.
    let take = SELECTION.with_borrow_mut(|s| {
        let list = s.entry(click.player).or_default();
        let i = match list.iter().position(|(p, _)| p.pos == point.pos) {
            Some(i) => i,
            None => {
                list.push((point, false));
                list.len() - 1
            }
        };
        let entry = &mut list[i];
        if !entry.0.kind.deposit_only() {
            entry.1 = !entry.1;
        }
        entry.1
    });
    send_selection(lv.host, player);
    let (key, color) = if take {
        ("create.mechanical_arm.extract_from", "#7FCDE0")
    } else {
        ("create.mechanical_arm.deposit_to", "#DDC166")
    };
    let name = coloured(mod_api::Text::translate(name_key, Vec::new()), "white");
    let line = coloured(mod_api::Text::translate(key, vec![name]), color);
    let _ = player.action_bar(&mut *lv.host, line);
    true
}

/// The arm points the player chose, to their client, which outlines them while the arm is in
/// hand (`ArmInteractionPointHandler.drawOutlines`).
pub const POINTS: &str = "create:arm_points";

fn send_selection(host: &mut dyn ServerHost, player: PlayerRef) {
    let list: Vec<([i32; 3], bool)> = SELECTION.with_borrow(|s| {
        s.get(&player.uuid())
            .map(|l| l.iter().map(|(p, take)| (p.pos, *take)).collect())
            .unwrap_or_default()
    });
    let mut data = Vec::new();
    arm::encode_points(&mut data, &list);
    net::send_to_player(host, player, POINTS, &data);
}

/// `text` in colour `color`.
fn coloured(text: mod_api::Text, color: &str) -> mod_api::Text {
    let mut style = mod_api::text::TextStyle::default();
    style.color = Some(color.into());
    text.with_style(style)
}

/// `ArmInteractionPointHandler.flushSettings` and `ArmPlacementPacket`: an arm a player places
/// takes the points they chose that lie within its range; the action bar counts them, or says
/// how many were out of range. The arm is saved for its block, which `Arms::placed` finds.
pub fn arm_placed(host: &mut dyn ServerHost, event: &mod_api::EntityPlaceEvent) {
    let Some(placer) = event.placer else {
        return;
    };
    let Some(id) = super::super::states::id(event.state) else {
        return;
    };
    let Some((Block::MechanicalArm, info)) =
        with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten()
    else {
        return;
    };
    let Some(player) = host
        .players()
        .server_players()
        .into_iter()
        .find(|p| p.entity() == placer)
    else {
        return;
    };
    let uuid = player.uuid();
    let Some(mut chosen) = SELECTION.with_borrow_mut(|s| s.remove(&uuid)) else {
        return;
    };
    send_selection(host, player);
    let pos = [event.pos.x, event.pos.y, event.pos.z];
    // `BlockPos.closerThan(pos, range)`.
    let near = |p: [i32; 3]| {
        let d = [p[0] - pos[0], p[1] - pos[1], p[2] - pos[2]];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < ARM_RANGE * ARM_RANGE
    };
    let before = chosen.len();
    chosen.retain(|(p, _)| near(p.pos));
    let removed = before - chosen.len();
    let inputs = chosen.iter().filter(|(_, take)| *take).count();
    let line = if removed > 0 {
        Some(coloured(
            mod_api::Text::translate(
                "create.mechanical_arm.points_outside_range",
                vec![mod_api::Text::literal(removed.to_string())],
            ),
            "red",
        ))
    } else {
        (!chosen.is_empty()).then(|| {
            coloured(
                mod_api::Text::translate(
                    "create.mechanical_arm.summary",
                    vec![
                        mod_api::Text::literal(inputs.to_string()),
                        mod_api::Text::literal((chosen.len() - inputs).to_string()),
                    ],
                ),
                "white",
            )
        })
    };
    if let Some(line) = line {
        let _ = player.action_bar(host, line);
    }
    let Some(dim) = placer
        .level(&*host)
        .and_then(|l| l.dimension().id(&*host))
        .and_then(|d| super::super::world::level_index(&d))
    else {
        return;
    };
    let (inputs, outputs): (Vec<_>, Vec<_>) = chosen.into_iter().partition(|(_, take)| *take);
    let mut arm = arm::Arm::new(info.ceiling);
    arm.set_points(
        pos,
        inputs.into_iter().map(|(p, _)| p).collect(),
        outputs.into_iter().map(|(p, _)| p).collect(),
    );
    with_world(|w| {
        w.dims[dim].arms.saved.insert(pos, arm);
    });
}

/// The ejector target a player chose, to their client (none: empty), which outlines it and
/// draws the arc while the ejector is in hand.
pub const EJECTOR_TARGET_CHANNEL: &str = "create:ejector_target";

/// Create's default `maxEjectorDistance`.
const MAX_EJECTOR_DISTANCE: i32 = 32;

thread_local! {
    /// The block each player chose as the target of the next ejector they place
    /// (`EjectorTargetHandler.currentSelection`, kept on the server here).
    static EJECTOR_TARGET: std::cell::RefCell<HashMap<u128, [i32; 3]>> =
        std::cell::RefCell::default();
    /// Ejectors just placed that turn to face their target once their block is there
    /// (`EjectorPlacementPacket.handle`), by level.
    static EJECTOR_FACING: std::cell::RefCell<Vec<(usize, [i32; 3], Dir)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// `EjectorTargetHandler.rightClickingBlocksSelectsThem`: sneaking with an ejector in the main
/// hand, a click on any block makes it the target of the next ejector placed. On a client it
/// only keeps the ejector from being placed there.
pub fn select_ejector_target(
    host: mod_api::HostSide,
    event: &mut mod_api::RightClickBlockEvent<'_>,
) -> mod_api::Flow {
    let click = event.click;
    let is_ejector = |host: &dyn mod_api::Host| {
        click.hand == mod_api::Hand::Main
            && click
                .stack
                .is_some_and(|s| items::id_of(host, s) == "create:weighted_ejector")
            && host.player(click.player).is_some_and(|p| {
                crate::glue::items::sneaking(host, p) && p.entity().spectator(host) != Some(true)
            })
    };
    let selected = match host {
        mod_api::HostSide::Client(host) => is_ejector(&*host),
        mod_api::HostSide::Server(host) => {
            let selected = is_ejector(&*host);
            if selected {
                EJECTOR_TARGET.with_borrow_mut(|t| t.insert(click.player, click.pos));
                if let Some(p) = host.player(click.player) {
                    let data: Vec<u8> = click.pos.iter().flat_map(|v| v.to_le_bytes()).collect();
                    net::send_to_player(host, p, EJECTOR_TARGET_CHANNEL, &data);
                    let line = coloured(
                        mod_api::Text::translate("create.weighted_ejector.target_set", Vec::new()),
                        "gold",
                    );
                    let _ = p.action_bar(host, line);
                }
            }
            selected
        }
    };
    if !selected {
        return mod_api::Flow::Continue;
    }
    *event.cancellation_result = mod_api::InteractionResult::Success;
    mod_api::Flow::Cancel
}

/// `leftClickingBlocksDeselectsThem`: with the arm item in the main hand, a left click on a
/// chosen point drops it (`ArmInteractionPointHandler`); sneaking with an ejector, one on the
/// chosen target drops it (`EjectorTargetHandler`). The click is then taken: nothing is dug.
pub fn left_click(host: &mut dyn ServerHost, pos: [i32; 3]) -> mod_api::Flow {
    let Some(player) = host.players().acting_player() else {
        return mod_api::Flow::Continue;
    };
    let held = crate::glue::items::held(&*host, player, mod_api::Hand::Main);
    let uuid = player.uuid();
    if is_arm_item(&*host, held.as_ref()) {
        let removed = SELECTION.with_borrow_mut(|s| {
            let list = s.get_mut(&uuid)?;
            let i = list.iter().position(|(p, _)| p.pos == pos)?;
            list.remove(i);
            Some(())
        });
        if removed.is_some() {
            send_selection(host, player);
            return mod_api::Flow::Cancel;
        }
        return mod_api::Flow::Continue;
    }
    let ejector = held
        .as_ref()
        .is_some_and(|s| items::id_of(&*host, s) == "create:weighted_ejector");
    if ejector
        && crate::glue::items::sneaking(&*host, player)
        && EJECTOR_TARGET.with_borrow(|t| t.get(&uuid) == Some(&pos))
    {
        EJECTOR_TARGET.with_borrow_mut(|t| t.remove(&uuid));
        net::send_to_player(host, player, EJECTOR_TARGET_CHANNEL, &[]);
        return mod_api::Flow::Cancel;
    }
    mod_api::Flow::Continue
}

/// `EjectorTargetHandler.getValidTargetDirection`: the way an ejector at `pos` faces to throw
/// at `target`, straight along x or z within the maximum distance, not right above or below.
fn ejector_direction(pos: [i32; 3], target: [i32; 3]) -> Option<Dir> {
    let (dx, dz) = (target[0] - pos[0], target[2] - pos[2]);
    if (dx, dz) == (0, 0) || dx.abs() > MAX_EJECTOR_DISTANCE || dz.abs() > MAX_EJECTOR_DISTANCE {
        return None;
    }
    match (dx, dz) {
        (0, _) => Some(if dz < 0 { Dir::North } else { Dir::South }),
        (_, 0) => Some(if dx < 0 { Dir::West } else { Dir::East }),
        _ => None,
    }
}

/// `EjectorTargetHandler.flushSettings` and `EjectorPlacementPacket`: an ejector a player
/// places throws at the target they chose (it turns to face it) when that lies straight along
/// x or z within range; the action bar says where, or why not.
pub fn ejector_placed(host: &mut dyn ServerHost, event: &mod_api::EntityPlaceEvent) {
    let Some(placer) = event.placer else {
        return;
    };
    let Some(id) = super::super::states::id(event.state) else {
        return;
    };
    let Some(Block::WeightedEjector) = with_world(|w| w.table.get(id).map(|(b, _)| b)).flatten()
    else {
        return;
    };
    let Some(player) = host
        .players()
        .server_players()
        .into_iter()
        .find(|p| p.entity() == placer)
    else {
        return;
    };
    let pos = [event.pos.x, event.pos.y, event.pos.z];
    let target = EJECTOR_TARGET.with_borrow_mut(|t| t.remove(&player.uuid()));
    net::send_to_player(host, player, EJECTOR_TARGET_CHANNEL, &[]);
    let direction = target.and_then(|t| ejector_direction(pos, t));
    let (Some(target), Some(direction)) = (target, direction) else {
        let key = if target.is_none() {
            "create.weighted_ejector.no_target"
        } else {
            "create.weighted_ejector.target_not_valid"
        };
        let line = coloured(mod_api::Text::translate(key, Vec::new()), "white");
        let _ = player.action_bar(host, line);
        return;
    };
    let line = coloured(
        mod_api::Text::translate(
            "create.weighted_ejector.targeting",
            target
                .iter()
                .map(|v| mod_api::Text::literal(v.to_string()))
                .collect(),
        ),
        "green",
    );
    let _ = player.action_bar(host, line);
    let Some(dim) = placer
        .level(&*host)
        .and_then(|l| l.dimension().id(&*host))
        .and_then(|d| super::super::world::level_index(&d))
    else {
        return;
    };
    let h = (target[0] - pos[0] + target[2] - pos[2]).abs();
    let v = target[1] - pos[1];
    let mut e = ejector::Ejector::default();
    e.set_target(h, v);
    with_world(|w| {
        w.dims[dim].ejector_saved.insert(pos, e);
    });
    EJECTOR_FACING.with_borrow_mut(|f| f.push((dim, pos, direction)));
}

/// Ejectors placed this tick turn to face their target.
fn turn_placed_ejectors(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let due: Vec<([i32; 3], Dir)> = EJECTOR_FACING.with_borrow_mut(|f| {
        let (mine, rest): (Vec<_>, Vec<_>) = f.drain(..).partition(|(d, _, _)| *d == dim);
        *f = rest;
        mine.into_iter().map(|(_, p, d)| (p, d)).collect()
    });
    for (pos, facing) in due {
        if info_at(lv, pos).is_some_and(|(b, _)| b == Block::WeightedEjector) {
            lv.set_property(
                pos,
                "facing",
                super::super::kinetics::dir_name(facing),
                super::super::server::update_all(),
            );
        }
    }
}
