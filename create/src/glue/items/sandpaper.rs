//! Sand paper (`SandPaperItem`): used with a polishable item in the other hand (a
//! `create:sandpaper_polishing` recipe's ingredient), it takes one into itself and is held for
//! 32 ticks, then gives back the recipe's result and wears by one; let go early, it gives the
//! item back. Used on weathered or waxed copper it scrapes it as an axe would
//! (`canPerformAction`: `AXE_SCRAPE`, `AXE_WAX_OFF`, which the decor port's copper does for
//! axes). The item being polished is kept in the stack's custom data
//! (`create:sand_paper_polishing`).
//!
//! With nothing polishable in the other hand, it takes one off a polishable item lying within a
//! block of the point looked at and 3 of the player (`getEntitiesOfClass` round the
//! `getPlayerPOVHitResult`). While it is used, a crumb of the item flies from the mouth each tick
//! and the sanding sound plays every 7 ticks (`triggerUseEffects`), and a finished polish
//! scatters 20 (`spawnParticles`).

use super::{custom, held, set_custom, set_held};
use mod_api::{
    ClientHost, Hand, HostSide, InteractionResult, ItemPart, ItemStack, PlayerRef,
    ReleaseUsingEvent, ServerHost, Tag, UseAnimation, UseAnimationEvent, UseDurationEvent,
    UseItemEvent, UseOnEvent,
};

pub(super) const IDS: [&str; 2] = ["create:sand_paper", "create:red_sand_paper"];
const POLISHING: &str = "create:sand_paper_polishing";

pub(super) struct SandPaper;

fn other(hand: Hand) -> Hand {
    match hand {
        Hand::Main => Hand::Off,
        Hand::Off => Hand::Main,
    }
}

/// The item a stack of sand paper is polishing.
fn polishing(host: &dyn mod_api::Host, stack: &ItemStack) -> Option<ItemStack> {
    match custom(stack).get(&POLISHING.into()) {
        Some(Tag::Compound(c)) => ItemStack::from_nbt(host, c),
        _ => None,
    }
}

/// `SandPaperPolishingRecipe.applyPolish`'s result for `item`.
fn polished(host: &dyn mod_api::Host, item: &ItemStack) -> Option<ItemStack> {
    let id = item.item().id(host);
    let (out, n) = crate::glue::processing::recipes::with(|r| {
        let i = r.polishing.find_single(&id)?;
        let o = r.polishing.recipes[i as usize].results.first()?;
        Some((o.item.clone(), o.count))
    })??;
    crate::glue::packages::body::new_stack(host, &out, u32::from(n.max(1)))
}

#[mod_api::horizon_part]
impl ItemPart for SandPaper {
    /// `getUseDuration`: 32.
    fn use_duration(&self, _host: &dyn mod_api::Host, event: &mut UseDurationEvent<'_>) {
        event.duration = 32;
    }

    /// `getUseAnimation`: eating's.
    fn use_animation(&self, _host: &dyn ClientHost, event: &mut UseAnimationEvent<'_>) {
        *event.animation = UseAnimation::Eat;
    }

    /// `use`: already polishing, it goes on; else one of the other hand's polishable items is
    /// taken in and the use starts; else it fails.
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        match host {
            // The server decides: an item on the ground may be taken (the client can't read an
            // item entity's stack).
            HostSide::Client(_) => None,
            HostSide::Server(host) => {
                if polishing(&*host, event.stack).is_some() {
                    return None;
                }
                let player = host.player(event.player)?;
                let hand = other(event.hand);
                let Some(item) =
                    held(&*host, player, hand).filter(|i| polished(&*host, i).is_some())
                else {
                    return pick_up(host, player, event);
                };
                let one = crate::glue::packages::body::with_count(&item, 1)?;
                let nbt = one.to_nbt(&*host)?;
                let mut paper = event.stack.clone();
                set_custom(&mut paper, &[(POLISHING, Some(Tag::Compound(nbt)))]);
                set_held(host, player, event.hand, Some(paper));
                set_held(host, player, hand, item.shrink(1));
                None
            }
        }
    }

    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => scrape(host, event),
            HostSide::Client(host) => {
                let state = host
                    .level(host.dimension())?
                    .block_state(&*host, event.pos)?;
                let text = host.block_state_string(state)?;
                let (name, _) = super::super::states::split(&text);
                let to = scraped(name)?;
                host.block_key(&to).map(|_| InteractionResult::Success)
            }
        }
    }

    /// `releaseUsing`: the item goes back to the player.
    fn release_using(&self, host: HostSide, event: &mut ReleaseUsingEvent<'_>) {
        let HostSide::Server(host) = host else {
            return;
        };
        let Some(player) = user(host, event.entity).or_else(|| host.players().acting_player())
        else {
            return;
        };
        give_back(host, player, false);
    }
}

/// `triggerUseEffects`, each tick of the use (`living_entity_use_item_tick`, as the item's own
/// `onUseTick` is not asked of a templated item): a crumb of the item being polished
/// (`spawnItemParticles(polishing, 1)`), and the sanding sound (`getEatingSound`) on the 6th tick
/// and every 7th after.
pub(super) fn use_tick(
    host: &mut dyn ServerHost,
    stack: &ItemStack,
    entity: Option<i32>,
    remaining: i32,
) {
    if !IDS.contains(&stack.item().id(&*host).as_str()) {
        return;
    }
    let Some(item) = polishing(&*host, stack) else {
        return;
    };
    // The server's event names no entity: the player holding this stack.
    let Some(player) = user(&*host, entity).or_else(|| {
        host.players().server_players().into_iter().find(|p| {
            [Hand::Main, Hand::Off]
                .into_iter()
                .any(|h| held(&*host, *p, h).as_ref() == Some(stack))
        })
    }) else {
        return;
    };
    let entity = player.entity();
    let (Some(level), Some(at), Some(eye), Some([yaw, pitch])) = (
        entity.level(&*host),
        entity.position(&*host),
        entity.eye_position(&*host),
        entity.rotation(&*host),
    ) else {
        return;
    };
    use mod_api::RandomSource;
    let r: [f32; 6] = level
        .with_random(&mut *host, |r| std::array::from_fn(|_| r.next_float()))
        .unwrap_or([0.5; 6]);
    // `spawnItemParticles`: a velocity and a spot before the mouth, turned with the head.
    let turn = |v: [f64; 3]| {
        let (sx, cx) = (-f64::from(pitch)).to_radians().sin_cos();
        let (sy, cy) = (-f64::from(yaw)).to_radians().sin_cos();
        // `xRot`, then `yRot`.
        let v = [v[0], v[1] * cx + v[2] * sx, v[2] * cx - v[1] * sx];
        [v[0] * cy + v[2] * sy, v[1], v[2] * cy - v[0] * sy]
    };
    let velocity = turn([
        (f64::from(r[0]) - 0.5) * 0.1,
        f64::from(r[1]) * 0.1 + 0.1,
        0.0,
    ]);
    let spot = turn([
        (f64::from(r[2]) - 0.5) * 0.3,
        -f64::from(r[3]) * 0.6 - 0.3,
        0.6,
    ]);
    let from = [at[0] + spot[0], eye[1] + spot[1], at[2] + spot[2]];
    if let Some(key) = ITEM_PARTICLE.get() {
        let options = mod_api::ParticleOptions::item_stack(*key, item);
        let v = [velocity[0], velocity[1] + 0.05, velocity[2]];
        let _ = level.send_particles(&mut *host, &options, None, from, v, 1.0, 0);
    }
    let ticks = 32 - remaining;
    if (ticks - 6) % 7 == 0 {
        let _ = level.play_sound(
            &mut *host,
            "create:sanding_short",
            at,
            mod_api::SoundSource::Players,
            0.9 + 0.2 * r[4],
            r[5] * 0.2 + 0.9,
        );
    }
}

/// The `minecraft:item` particle type.
pub(super) static ITEM_PARTICLE: std::sync::OnceLock<mod_api::ParticleKey> =
    std::sync::OnceLock::new();

/// `use` with nothing to polish in the other hand: the first living polishable item within a
/// block of the point looked at (`getPlayerPOVHitResult`, as far as the player reaches blocks)
/// and 3 of the player gives one of itself to the paper.
fn pick_up(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    event: &mut UseItemEvent<'_>,
) -> Option<InteractionResult> {
    let entity = player.entity();
    let level = entity.level(&*host)?;
    let reach = host
        .living(entity)
        .and_then(|l| l.attribute_value(&*host, "minecraft:block_interaction_range"))
        .unwrap_or(4.5);
    let hit = entity.pick(&*host, reach)?.location;
    let me = entity.position(&*host)?;
    let found = level
        .entities_in(&*host, hit.map(|v| v - 1.0), hit.map(|v| v + 1.0))
        .unwrap_or_default()
        .into_iter()
        .find_map(|e| {
            let item = host.item_entity(e)?;
            let at = e.position(&*host)?;
            let d = [0, 1, 2].map(|i| at[i] - me[i]);
            if (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() > 3.0 {
                return None;
            }
            let stack = item.stack(&*host)?;
            polished(&*host, &stack)
                .is_some()
                .then_some((e, item, stack))
        });
    let Some((e, item, stack)) = found else {
        return Some(InteractionResult::Fail);
    };
    let one = crate::glue::packages::body::with_count(&stack, 1)?;
    let nbt = one.to_nbt(&*host)?;
    let mut paper = event.stack.clone();
    set_custom(&mut paper, &[(POLISHING, Some(Tag::Compound(nbt)))]);
    set_held(host, player, event.hand, Some(paper));
    match stack.shrink(1) {
        Some(rest) => {
            let _ = item.set_stack(host, rest);
        }
        None => {
            e.discard(host);
        }
    }
    None
}

/// The player using an item, by entity id.
fn user(host: &dyn ServerHost, entity: Option<i32>) -> Option<PlayerRef> {
    let id = entity?;
    host.players()
        .server_players()
        .into_iter()
        .find(|p| i64::from(p.entity().id()) == i64::from(id))
}

/// The held sand paper's item, given back as it is (`releaseUsing`) or polished
/// (`finishUsingItem`, the paper wearing by one).
fn give_back(host: &mut dyn ServerHost, player: PlayerRef, finished: bool) -> Option<ItemStack> {
    let (hand, paper) = [Hand::Main, Hand::Off].into_iter().find_map(|h| {
        let s = held(&*host, player, h)?;
        IDS.contains(&s.item().id(&*host).as_str())
            .then_some((h, s))
            .filter(|(_, s)| polishing(&*host, s).is_some())
    })?;
    let item = polishing(&*host, &paper)?;
    let mut paper = paper;
    set_custom(&mut paper, &[(POLISHING, None)]);
    let given = if finished {
        polished(&*host, &item)
    } else {
        Some(item)
    };
    if let Some(g) = given {
        super::give_back(host, player, g);
    }
    let paper = if finished && !super::creative(&*host, player) {
        // `hurtAndBreak(1)`: eight uses.
        let damage = paper.damage() + 1;
        if damage >= 8 {
            None
        } else {
            paper.set_damage(damage);
            Some(paper)
        }
    } else {
        Some(paper)
    };
    set_held(host, player, hand, paper.clone());
    paper
}

/// `finishUsingItem`, from `living_entity_use_item_finish`: the result of polishing, and the
/// worn paper left in the hand.
pub(super) fn finished(
    host: &mut dyn ServerHost,
    event: &mut mod_api::LivingEntityUseItemFinishEvent<'_>,
) {
    if !IDS.contains(&event.stack.item().id(&*host).as_str()) {
        return;
    }
    if polishing(&*host, event.stack).is_none() {
        return;
    }
    // The entity is not named on a server: the acting player is the one using it.
    let Some(player) = user(&*host, event.entity).or_else(|| host.players().acting_player()) else {
        return;
    };
    // `spawnParticles`: 20 crumbs of the item half a block before the eyes, each thrown up to an
    // eighth along each axis.
    if let (Some(item), Some(key)) = (polishing(&*host, event.stack), ITEM_PARTICLE.get()) {
        let entity = player.entity();
        if let (Some(level), Some(eye), Some(look)) = (
            entity.level(&*host),
            entity.eye_position(&*host),
            entity.view_vector(&*host),
        ) {
            use mod_api::RandomSource;
            let at = [0, 1, 2].map(|i| eye[i] + look[i] * 0.5);
            let options = mod_api::ParticleOptions::item_stack(*key, item);
            for _ in 0..20 {
                let v: [f64; 3] = level
                    .with_random(&mut *host, |r| {
                        std::array::from_fn(|_| (f64::from(r.next_float()) - 0.5) * 2.0 / 8.0)
                    })
                    .unwrap_or([0.0; 3]);
                let _ = level.send_particles(&mut *host, &options, None, at, v, 1.0, 0);
            }
        }
    }
    *event.result = give_back(host, player, true);
}

/// The block a scrape (`AXE_SCRAPE`: an age back) or a wax-off (`AXE_WAX_OFF`) turns copper
/// block `name` into, by its name as vanilla's and Create's copper are named.
fn scraped(name: &str) -> Option<String> {
    let (ns, path) = name.split_once(':')?;
    if !path.contains("copper") {
        return None;
    }
    let back = if let Some(rest) = path.strip_prefix("waxed_") {
        rest.to_owned()
    } else if let Some(rest) = path.strip_prefix("oxidized_") {
        format!("weathered_{rest}")
    } else if let Some(rest) = path.strip_prefix("weathered_") {
        format!("exposed_{rest}")
    } else {
        // `exposed_copper` steps back to `copper_block`.
        let rest = path.strip_prefix("exposed_")?;
        if rest == "copper" {
            "copper_block".to_owned()
        } else {
            rest.to_owned()
        }
    };
    Some(format!("{ns}:{back}"))
}

/// `SandPaperItem.useOn`: copper scraped an age back, else its wax taken off, with the sanding
/// sound and the scrape's or wax-off's particles; the paper wears by one.
fn scrape(host: &mut dyn ServerHost, event: &UseOnEvent<'_>) -> Option<InteractionResult> {
    let player = host.player(event.player)?;
    let dimension = player.dimension(&*host)?.to_string();
    let mut lv = super::super::server::ServerLevel::new(host, &dimension)?;
    let state = lv.block_state(event.pos)?;
    let text = lv.host.block_state_string(state)?;
    let (name, props) = super::super::states::split(&text);
    let to = scraped(name)?;
    let key = lv.host.block_key(&to)?;
    let facts = mod_api::HostBlockStates::block_states(&*lv.host);
    let mut next = lv.host.block_state_default(key)?;
    for (k, v) in &props {
        if let Ok(s) = next.with(&facts, k, v) {
            next = s;
        }
    }
    let wax = name
        .split_once(':')
        .is_some_and(|(_, p)| p.starts_with("waxed_"));
    lv.set_state(event.pos, next, super::super::server::update_all());
    let pitch = {
        use mod_api::RandomSource;
        let r = lv
            .level
            .with_random(&mut *lv.host, |r| r.next_float())
            .unwrap_or(0.5);
        1.0 + (r * 0.5 - 1.0) / 5.0
    };
    let at = event.pos.map(|v| f64::from(v) + 0.5);
    lv.level.play_sound(
        &mut *lv.host,
        "create:sanding_long",
        at,
        mod_api::SoundSource::Players,
        1.0,
        pitch,
    );
    let _ = lv
        .level
        .level_event(&mut *lv.host, if wax { 3004 } else { 3005 }, event.pos, 0);
    if !super::creative(&*lv.host, player) {
        let mut paper = event.stack.clone();
        let damage = paper.damage() + 1;
        let paper = if damage >= 8 {
            None
        } else {
            paper.set_damage(damage);
            Some(paper)
        };
        set_held(lv.host, player, event.hand, paper);
    }
    Some(InteractionResult::Success)
}

#[cfg(test)]
mod tests {
    use super::scraped;

    #[test]
    fn copper_scrapes_an_age_back_and_loses_its_wax() {
        assert_eq!(
            scraped("create:weathered_copper_shingles").as_deref(),
            Some("create:exposed_copper_shingles")
        );
        assert_eq!(
            scraped("minecraft:exposed_copper").as_deref(),
            Some("minecraft:copper_block")
        );
        assert_eq!(
            scraped("minecraft:waxed_oxidized_cut_copper").as_deref(),
            Some("minecraft:oxidized_cut_copper")
        );
        assert_eq!(scraped("create:copper_tiles"), None);
        assert_eq!(scraped("minecraft:stone"), None);
    }
}
