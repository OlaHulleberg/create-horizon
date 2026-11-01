// The deployer's fake player (`DeployerHandler.activate`) where it needs the dispenser's private
// behaviours and entity lookups: an item used on the block it reaches, and its punch at the
// entities there. Create's rules are in `create_engine::deployer`.
use crate::game::create::*;
use create_engine::deployer::{self, Use};
use create_engine::kinetics::Dir;
use horizon_client::items::ItemStack;

/// The held item used on the block the deployer at `pos` reaches (toward `facing`): an axe, hoe
/// or shovel transforms it (`useOn`), a spawn egg spawns its mob (`SpawnEggItem.useOn`), other
/// items act as the dispenser's own behaviours (`useOn` / `use` as a dispenser would). What is
/// left of the stack, or None for an item without such a use.
pub(crate) fn use_item_at(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    facing: Dir,
    stack: ItemStack,
) -> Option<Option<ItemStack>> {
    let transformer = game
        .items()
        .get(&stack.item)
        .is_some_and(|i| i.block_transformer.is_some());
    match deployer::item_use(&stack.item, transformer) {
        Use::Transform => return Some(transform(game, pos, facing, stack)),
        Use::SpawnEgg => return Some(spawn_egg(game, pos, facing, stack)),
        Use::Dispense => {}
        Use::Nothing => return None,
    }
    let o = facing.offset();
    let at = (pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]);
    let facing = horizon_core::geometry::Direction::ALL[facing as usize];
    let used = match &*stack.item {
        "minecraft:bucket" => game.dispense_empty_bucket(at, facing, stack),
        "minecraft:water_bucket" | "minecraft:lava_bucket" | "minecraft:powder_snow_bucket" => {
            game.dispense_filled_bucket(at, facing, stack)
        }
        "minecraft:flint_and_steel" => game.dispense_flint_and_steel(at, facing, stack),
        "minecraft:bone_meal" => game.dispense_bone_meal(at, facing, stack),
        "minecraft:glass_bottle" => game.dispense_glass_bottle(at, facing, stack),
        "minecraft:glowstone" => game.dispense_glowstone(at, facing, stack),
        "minecraft:shears" => game.dispense_shears(at, facing, stack),
        "minecraft:honeycomb" => game.dispense_honeycomb(at, facing, stack),
        "minecraft:brush" => Ok(game.dispense_brush(at, facing, stack)),
        _ => return None,
    };
    used.ok().map(|(left, _)| left)
}

/// `BlockTransformer.transformBlock` for the fake player's axe, hoe or shovel on the block it
/// reaches, clicked on the face toward the deployer (`Game::use_block_transformer` without the
/// player's hand, statistics and shield). Held stacks carry no damage here, so it does not wear.
fn transform(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    facing: Dir,
    stack: ItemStack,
) -> Option<ItemStack> {
    use horizon_client::block_transform::Around;
    struct Column<'w> {
        world: &'w horizon_client::world::World,
        registry: &'w horizon_world::registry::BlockRegistry,
        pos: [i32; 3],
    }
    impl Around for Column<'_> {
        fn state(&self, o: [i32; 3]) -> horizon_world::chunk::BlockState {
            let id = self
                .world
                .block(self.pos[0] + o[0], self.pos[1] + o[1], self.pos[2] + o[2]);
            self.registry
                .state(id)
                .cloned()
                .unwrap_or_else(|| horizon_world::chunk::BlockState {
                    name: "minecraft:air".into(),
                    properties: Default::default(),
                })
        }
    }
    let Some(id) = game
        .items()
        .get(&stack.item)
        .and_then(|i| i.block_transformer.clone())
    else {
        return Some(stack);
    };
    let at = deployer::clicked(pos, facing);
    let face = deployer::clicked_face(facing);
    let face_name = match face {
        Dir::Down => "down",
        Dir::Up => "up",
        Dir::North => "north",
        Dir::South => "south",
        Dir::West => "west",
        Dir::East => "east",
    };
    let registry = game.pack.assets.registry;
    let around = Column {
        world: &game.world,
        registry,
        pos: at,
    };
    let Some((new, sound, loot, particle)) = game
        .res
        .transformers()
        .apply(game.items(), registry, &id, face_name, &around)
        .and_then(|(_, t, state)| {
            let new = registry.resolve(&state).ok()?;
            Some((new, t.sound.clone(), t.loot.clone(), t.particle.clone()))
        })
    else {
        return Some(stack);
    };
    if game.level_set(at, new).is_err() {
        return Some(stack);
    }
    game.transform_particle(&particle, at);
    if loot.as_deref() == Some("minecraft:till/rooted_dirt") {
        let o = face.offset();
        game.pop_resource(
            [at[0] + o[0], at[1] + o[1], at[2] + o[2]],
            ItemStack::new("minecraft:hanging_roots", 1),
        );
    }
    let center = at.map(|v| f64::from(v) + 0.5);
    game.audio.play(&sound, Some(center), 1.0, 1.0);
    game.post_block_game_event("minecraft:block_change", at, None, Some(new));
    Some(stack)
}

/// `SpawnEggItem.useOn` for the fake player: on a spawner the egg sets its mob (the fake player
/// is no op, so not the egg's data); else its mob comes where `deployer::spawn_egg_at` says,
/// moved down onto the ground (`EntityType.spawn`, `tryMoveDown`; further down when
/// `deployer::spawn_egg_offsets_more`), and one egg is used.
fn spawn_egg(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    facing: Dir,
    stack: ItemStack,
) -> Option<ItemStack> {
    let at = deployer::clicked(pos, facing);
    let (kind, _) = horizon_client::block_entity_item::spawn_egg_on_spawner(&stack, false);
    if let Some(done) = game.use_spawn_egg_on_spawner(at, &kind, None) {
        return if done {
            super::shrink(stack)
        } else {
            Some(stack)
        };
    }
    let kind = stack.item.trim_end_matches("_spawn_egg").to_owned();
    // EntityType.canSpawn: no monsters in peaceful.
    let id = format!("minecraft:{}", kind.trim_start_matches("minecraft:"));
    if game.ui.difficulty == 0
        && let Some(t) = game.res.mob_types().get(&id)
        && !horizon_client::mobs::spawn_rules::allowed_in_peaceful(t.species)
    {
        return Some(stack);
    }
    let state = game.world.block(at[0], at[1], at[2]);
    let empty = game
        .res
        .behaviours()
        .collision_shape(state)
        .is_some_and(|s| s.is_empty());
    let p = deployer::spawn_egg_at(pos, facing, empty);
    let spawned = if deployer::spawn_egg_offsets_more(facing, empty) {
        spawn_mob_offset_more(game, &id, p)
    } else {
        game.spawn_mob_at(&kind, (p[0], p[1], p[2]), true)
    };
    let Some(mob) = spawned else {
        return Some(stack);
    };
    // EntityType.create: `finalizeSpawn` first, the egg's components after.
    if let Some(m) = game.mobs.iter_mut().find(|m| m.id == mob) {
        horizon_client::entity_nbt::apply_item_components(m, &stack);
    }
    let _ = game.mob_ambient(mob);
    game.post_block_game_event("minecraft:entity_place", at, None, None);
    super::shrink(stack)
}

/// `EntityType.spawn(.., pos, SPAWN_EGG, true, true)` for the mob `name` at `p`
/// (`Dispenser::spawn_mob_at` with `shouldOffsetYMore`): `getYOffset(level, pos, true, box)`,
/// its box set on top of the block at `p` falls up to two blocks against the collision shapes
/// of that block and the one below it.
fn spawn_mob_offset_more(game: &mut crate::game::Game<'_>, name: &str, p: [i32; 3]) -> Option<u32> {
    use horizon_core::geometry::{Aabb, Vec3};
    use horizon_world::{
        block_collision_scan::BlockCollisionScan, collision_context::CollisionContext,
    };
    let t = game.res.mob_types().get(name)?.clone();
    let h = t.width / 2.0;
    let (x, z) = (f64::from(p[0]) + 0.5, f64::from(p[2]) + 0.5);
    let y = f64::from(p[1]) + 1.0;
    let entity = Aabb::new(
        Vec3::new(x - h, y, z - h),
        Vec3::new(x + h, y + t.height, z + h),
    );
    // `new AABB(pos).expandTowards(0, -1, 0)`.
    let cells = Aabb::new(
        Vec3::new(f64::from(p[0]), f64::from(p[1]) - 1.0, f64::from(p[2])),
        Vec3::new(
            f64::from(p[0]) + 1.0,
            f64::from(p[1]) + 1.0,
            f64::from(p[2]) + 1.0,
        ),
    );
    let mut scene = horizon_client::collisions::SceneCollisions {
        world: &game.world,
        binding: game.collision,
        subject: None,
    };
    let mut amount = -2.0;
    // Level.getCollisions(null, aabb): CollisionContext.empty().
    if let Ok(scan) = BlockCollisionScan::new(
        &mut scene,
        CollisionContext::empty(false),
        cells,
        false,
        1_000_000,
    ) {
        for hit in scan.flatten() {
            amount = hit
                .shape
                .collide(horizon_core::geometry::Axis::Y, entity, amount);
        }
    }
    let id = game.spawn_mob(name, [x, f64::from(p[1]) + 1.0 + amount, z], true)?;
    game.spawn_jockeys(id, false);
    Some(id)
}

/// `activateInner`'s entities in `Mode.PUNCH`: one of those in the block at `at` (any but a
/// contraption), at random (`level.random`), is attacked (`Player.attack`) for `damage` from
/// `from`, where the fake player stands; true when there was one, and the block is then left
/// alone. Only mobs take the hit here (`others_at` counts the rest, unharmed). The hit has no
/// attacker: Create's fake player is one, so mobs but creepers (`ignoreDeployerAttacks`, default
/// `CREEPERS`) turn on it and its kills count as a player's, but Horizon has no entity for it
/// and naming the host instead would set them on the host.
pub(crate) fn punch_entity_at(
    game: &mut crate::game::Game<'_>,
    at: [i32; 3],
    from: [f64; 3],
    damage: f32,
) -> bool {
    use horizon_core::random::RandomSource;
    let p = (at[0], at[1], at[2]);
    let targets = game.targets_at(p);
    let total = targets.len() + others_at(game, p);
    if total == 0 {
        return false;
    }
    let pick = game.world.random.next_int_bound(total as i32).unwrap_or(0) as usize;
    if let Some(super::Target::Mob(index)) = targets.get(pick) {
        let index = *index;
        let source = horizon_client::mobs::Damage {
            kind: horizon_client::mobs::DamageKind::PlayerAttack,
            attacker: None,
            from: Some(from),
        };
        let old = game.mobs[index].health;
        if game.mobs[index].damage(damage, source) && game.mobs[index].health != old {
            let id = game.mobs[index].id;
            game.mob_damage_event(id);
        }
    }
    true
}

/// The entities `targets_at` leaves out whose box meets the block at `p`
/// (`getEntitiesOfClass(Entity.class, new AABB(clickedPos))`): items, experience orbs,
/// projectiles, item frames, paintings, boats, minecarts, end crystals, and by their position
/// the entities Horizon keeps without simulating them.
fn others_at(game: &crate::game::Game<'_>, p: (i32, i32, i32)) -> usize {
    use super::super::{boats, minecarts};
    use horizon_core::geometry::{Aabb, Vec3};
    let block = super::block_box(p);
    let pair = |(lo, hi): ([f64; 3], [f64; 3])| {
        Aabb::new(
            Vec3::new(lo[0], lo[1], lo[2]),
            Vec3::new(hi[0], hi[1], hi[2]),
        )
    };
    // `EntityDimensions.makeBoundingBox`.
    let standing = |at: Vec3, width: f64, height: f64| {
        let h = width / 2.0;
        Aabb::new(
            Vec3::new(at.x - h, at.y, at.z - h),
            Vec3::new(at.x + h, at.y + height, at.z + h),
        )
    };
    let inside = |v: Vec3| {
        [v.x, v.y, v.z]
            .iter()
            .zip([p.0, p.1, p.2])
            .all(|(c, b)| c.floor() as i32 == b)
    };
    let boxes = game
        .dropped
        .iter()
        .map(|i| i.bounds())
        .chain(game.orbs.iter().map(|o| o.bounds()))
        .chain(game.creatures.projectiles.iter().map(|j| j.bounds()))
        .chain(game.item_frames.iter().map(|f| pair(f.bounds())))
        .chain(game.paintings.iter().map(|f| pair(f.bounds())))
        .chain(
            game.boats
                .iter()
                .map(|b| standing(b.position, boats::WIDTH, boats::HEIGHT)),
        )
        .chain(
            game.carts
                .iter()
                .map(|c| standing(c.position, minecarts::WIDTH, minecarts::HEIGHT)),
        )
        // EntityType.END_CRYSTAL is 2 by 2.
        .chain(game.crystals.iter().map(|c| standing(c.position, 2.0, 2.0)));
    boxes.filter(|b| super::overlaps(*b, block)).count()
        + game.others.iter().filter(|o| inside(o.position)).count()
}

/// `DeployerHandler.tryHarvestBlock` for the block at `at` a punch with `stack` broke:
/// `onDestroyedByPlayer` (`playerWillDestroy`, the break particles, the block's fluid left in its
/// place, as a waterlogged slab leaves its water), then, when `harvest` (`canHarvestBlock`), its
/// loot drawn with that tool (`Block.getDrops(.., player, prevHeldItem)`) and `spawnAfterBreak(..,
/// true)`: experience, an infested block's silverfish (held stacks carry no silk touch here). The
/// drops land where the block was; Create puts them in the deployer.
pub(crate) fn deployer_harvest(
    game: &mut crate::game::Game<'_>,
    dimension: &str,
    at: [i32; 3],
    stack: &horizon_client::items::ItemStack,
    harvest: bool,
) {
    // As `native_set_block`: only the server's level of this dimension.
    if (game.serving.is_none() && game.options.dedicated.is_none()) || dimension != game.dim.id() {
        return;
    }
    let state = game.world.block(at[0], at[1], at[2]);
    let name = game.res.facts().get(state).name.clone();
    // The block entity as it was, for what the drops copy from it.
    let key = (at[0], at[1], at[2]);
    if let Some(entity) = game.world.block_entities.get(&key).cloned() {
        let custom = game.world.block_names.get(&key).cloned();
        game.pending.record(at, &entity, &name, custom);
    } else if let Some(tag) = super::super::spill::saved_entity(&game.world, at) {
        game.pending.record_saved(at, &tag);
    }
    let removed = game.with_level(|b, level, ticks| {
        b.player_destroy_block(
            level,
            ticks,
            horizon_core::position::BlockPos::new(at[0], at[1], at[2]),
            false,
            harvest,
        )
    });
    if removed && harvest {
        for loot in game.block_loot_at(at, state, Some(stack)) {
            if &*loot.item != "minecraft:air" {
                game.pop_resource(at, loot);
            }
        }
        let center = at.map(|v| f64::from(v) + 0.5);
        if name.starts_with("minecraft:infested_") && game.ui.rules.bool("block_drops") {
            let feet = [center[0], f64::from(at[1]), center[2]];
            if let Some(id) = game.spawn_mob("minecraft:silverfish", feet, false) {
                game.mob_poof(id);
            }
        }
        let xp = game.block_experience(&name);
        if xp > 0 {
            game.spawn_orbs(center, xp);
        }
    }
    game.pending.clear(at);
    let _ = game.process_level_events();
}

/// `Level.destroyBlockProgress` for a deployer's fake player: the cracks at `at` (0..=9, else
/// none) for the host and the players near it.
pub(crate) fn deployer_crack(
    game: &mut crate::game::Game<'_>,
    breaker: i32,
    at: [i32; 3],
    stage: i32,
) {
    game.destroy_block_progress(breaker, at, stage);
    if let Some(host) = game.lan.as_ref() {
        use horizon_protocol::packets::play::{BlockDestruction, Clientbound};
        host.hub.broadcast_near(
            at.map(f64::from),
            64.0,
            &Clientbound::BlockDestruction(BlockDestruction {
                id: breaker,
                pos: at,
                progress: if (0..10).contains(&stage) {
                    stage as u8
                } else {
                    255
                },
            }),
            None,
        );
    }
}

/// `DeployerHandler.activateInner` in `Mode.PUNCH`: an entity in the block it reaches takes the
/// hit (`create_use::punch_entity_at`); else, with a held stack, the block there breaks in
/// strokes (`Deployer::punch`), the fake player's cracks on it, a fire in front of it goes out;
/// a broken block goes as `deployer_harvest` says.
pub(crate) fn deployer_punch(
    game: &mut crate::game::Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    facing: Dir,
) {
    use create_engine::deployer::{self, Punch};
    let Some(mut d) = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten() else {
        return;
    };
    let at = deployer::clicked(pos, facing);
    let held = d.held.as_ref().map(|h| h.0.clone());
    let bonus = held
        .as_deref()
        .and_then(|h| game.items().get(h))
        .map_or(0.0, |i| i.attack_damage);
    let (from, _) = deployer::ray(pos, facing);
    let damage = deployer::attack_damage(bonus);
    if punch_entity_at(game, at, from, damage) {
        return;
    }
    let breaker = deployer::breaker_id(pos);
    // `getShape(..).isEmpty()`: nothing to punch, and the breaking is forgotten.
    if game.outline_boxes(at).is_empty() {
        if d.breaking.take().is_some() {
            with_world(|w| w.set_deployer(dim, pos, d));
        }
        return;
    }
    // `BlockHelper.extinguishFire`: fire on the face toward the deployer goes out.
    let o = deployer::clicked_face(facing).offset();
    let front = [at[0] + o[0], at[1] + o[1], at[2] + o[2]];
    let registry = game.pack.assets.registry;
    let fire = registry
        .state(game.world.block(front[0], front[1], front[2]))
        .is_some_and(|s| s.name == "minecraft:fire");
    if fire {
        game.level_event_at(1009, front, 0);
        if let Some(air) = registry.default_state("minecraft:air") {
            game.native_set_block(
                dimension,
                front,
                air,
                crate::game::native_hooks::flags::UPDATE_ALL,
            );
        }
        return;
    }
    let Some(held) = held else {
        return;
    };
    let state = game.world.block(at[0], at[1], at[2]);
    let facts = game.res.facts().get(state);
    let (name, hardness, requires_tool) = (facts.name.clone(), facts.hardness, facts.requires_tool);
    let stack = horizon_client::items::ItemStack::new(held.as_str(), 1);
    let harvest = !requires_tool || game.items().correct_for_drops(Some(&stack), &name);
    let per_hit = deployer::destroy_progress(deployer::Dig {
        hardness,
        tool_speed: game.items().mining_speed(Some(&stack), &name),
        // Held stacks carry no enchantments here.
        efficiency: 0.0,
        harvest,
    });
    // `level.playSound(.., getHitSound(), NEUTRAL, .25, 1)`.
    let hit = game.res.facts().get(state).sounds.hit.clone();
    game.audio
        .play(&hit, Some(at.map(|v| f64::from(v) + 0.5)), 0.25, 1.0);
    let punched = d.punch(at, per_hit);
    with_world(|w| w.set_deployer(dim, pos, d));
    match punched {
        Punch::Broken => {
            deployer_crack(game, breaker, at, -1);
            deployer_harvest(game, dimension, at, &stack, harvest);
        }
        Punch::Crack(Some(stage)) => deployer_crack(game, breaker, at, stage),
        Punch::Crack(None) | Punch::Stop => {}
    }
}
