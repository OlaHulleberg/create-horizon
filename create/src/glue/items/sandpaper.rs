//! Sand paper (`SandPaperItem`): used with a polishable item in the other hand (a
//! `create:sandpaper_polishing` recipe's ingredient), it takes one into itself and is held for
//! 32 ticks, then gives back the recipe's result and wears by one; let go early, it gives the
//! item back. Used on weathered or waxed copper it scrapes it as an axe would
//! (`canPerformAction`: `AXE_SCRAPE`, `AXE_WAX_OFF`, which the decor port's copper does for
//! axes). The item being polished is kept in the stack's custom data
//! (`create:sand_paper_polishing`).
//!
//! Left out: picking the item to polish off the ground (`getEntitiesOfClass` round the block
//! looked at), and the hand's polishing particles and sounds.

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
            HostSide::Client(host) => {
                let player = host.player(event.player)?;
                let other = held(&*host, player, other(event.hand));
                (polishing(&*host, event.stack).is_none() && other.is_none())
                    .then_some(InteractionResult::Fail)
            }
            HostSide::Server(host) => {
                if polishing(&*host, event.stack).is_some() {
                    return None;
                }
                let player = host.player(event.player)?;
                let hand = other(event.hand);
                let Some(item) = held(&*host, player, hand) else {
                    return Some(InteractionResult::Fail);
                };
                if polished(&*host, &item).is_none() {
                    return Some(InteractionResult::Fail);
                }
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
    } else if let Some(rest) = path.strip_prefix("exposed_") {
        // `exposed_copper` steps back to `copper_block`.
        if rest == "copper" {
            "copper_block".to_owned()
        } else {
            rest.to_owned()
        }
    } else {
        return None;
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
