//! The cardboard sword: it bonks instead of hurting (`CardboardSwordItem`). Its knockback and
//! the slowness it gives animals are not ported, as in the legacy glue. Its click on a block
//! (`cardboardSwordsMakeNoiseOnClick`, a sound only the clicking client plays) is heard here
//! from the server, so the players near it hear it too: `left_click_block` is the server's.

use super::{Item, held, is};
use mod_api::{AttackEntityEvent, Flow, Hand, LeftClickBlockEvent, ServerHost, SoundSource};

/// `EntityTypeTags.ARTHROPOD` (vanilla 26.3's `entity_type/arthropod`).
const ARTHROPODS: &str = "minecraft:arthropod";

/// A left click on a block with the sword in the main hand: the bonk.
pub(super) fn left_click(host: &mut dyn ServerHost, event: &LeftClickBlockEvent) {
    let Some(player) = host.players().acting_player() else {
        return;
    };
    if !is(
        held(&*host, player, Hand::Main).as_ref(),
        Item::CardboardSword,
    ) {
        return;
    }
    let Some(level) = player.level(&*host) else {
        return;
    };
    let at = event.pos.map(|v| f64::from(v) + 0.5);
    level.play_sound(
        host,
        "create:cardboard_bonk",
        at,
        SoundSource::Players,
        0.5,
        1.85,
    );
}

/// `CardboardSwordItem.cardboardSwordsCannotHurtYou`: a cardboard sword in the main hand bonks a
/// living target other than an arthropod instead of hurting it.
pub(super) fn attack(host: &mut dyn ServerHost, event: &AttackEntityEvent<'_>) -> Flow {
    let Some(player) = host.players().acting_player() else {
        return Flow::Continue;
    };
    if !is(
        held(&*host, player, Hand::Main).as_ref(),
        Item::CardboardSword,
    ) || host.entity_type_in_tag(event.kind, ARTHROPODS)
    {
        return Flow::Continue;
    }
    let living = u32::try_from(event.entity)
        .ok()
        .and_then(|id| host.entity(id))
        .and_then(|e| host.living(e))
        .is_some();
    if !living {
        return Flow::Continue;
    }
    if let (Some(level), Some(at)) = (player.level(&*host), player.entity().position(&*host)) {
        level.play_sound(
            host,
            "create:cardboard_bonk",
            at,
            SoundSource::Players,
            0.75,
            1.85,
        );
    }
    Flow::Cancel
}
