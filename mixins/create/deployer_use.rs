// The deployer's fake player (`DeployerHandler.activate`) where it needs the dispenser's private
// behaviours and entity lookups: an item used on the block it reaches, and its punch at the
// entities there. Create's rules are in `create_engine::deployer`.
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
/// moved down onto the ground (`EntityType.spawn`, `tryMoveDown`), and one egg is used.
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
    let Some(mob) = game.spawn_mob_at(&kind, (p[0], p[1], p[2]), true) else {
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

/// `activateInner`'s entities in `Mode.PUNCH`: one of those in the block at `at`, at random
/// (`level.random`), is attacked (`Player.attack`) for `damage` from `from`, where the fake
/// player stands; true when there was one, and the block is then left alone. Only mobs take the
/// hit here; the hit has no attacker, so nothing turns on the player for it.
pub(crate) fn punch_entity_at(
    game: &mut crate::game::Game<'_>,
    at: [i32; 3],
    from: [f64; 3],
    damage: f32,
) -> bool {
    use horizon_core::random::RandomSource;
    let targets = game.targets_at((at[0], at[1], at[2]));
    if targets.is_empty() {
        return false;
    }
    let pick = game
        .world
        .random
        .next_int_bound(targets.len() as i32)
        .unwrap_or(0) as usize;
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
