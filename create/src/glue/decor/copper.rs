//! Create's copper shingles and tiles (`CopperBlockSet`): they weather into Create's next age
//! (`ChangeOverTimeBlock.changeOverTime` with `CopperRegistries.addWeathering`), an axe scrapes
//! them back an age or takes their wax off (`AxeItem`'s scrape and wax-off, through
//! `getToolModifiedState`), and honeycomb waxes them (`HoneycombItem.useOn`). Vanilla's code
//! steps only vanilla's copper; these are Create's chains, from the engine.

use super::super::server::ServerLevel;
use super::super::states;
use super::{create_block, player_dimension, use_one};
use create_engine::Block;
use create_engine::decor;
use mod_api::{
    BlockState, Flow, GameEventContext, Host, HostSide, InteractionResult, ItemPart, LevelHost,
    LevelWriteHost, RandomSource, RandomTickEvent, Registrar, ToolAbility, ToolModifiedStateEvent,
    UseOnEvent,
};

pub(super) fn register(r: &mut dyn Registrar) {
    for &block in Block::ALL.iter() {
        if weathering(block) || decor::copper_unwaxed(block).is_some() {
            r.block_part(states::key(block), Copper);
        }
    }
    let honeycomb = r.item_key("minecraft:honeycomb");
    r.item_part(honeycomb, Honeycomb);
}

/// One of Create's weathering copper blocks: an age in a chain, not waxed.
fn weathering(b: Block) -> bool {
    decor::copper_unwaxed(b).is_none()
        && (decor::copper_next(b).is_some() || decor::copper_previous(b).is_some())
}

/// `WeatheringCopper.getAge` of one of Create's: the steps back to its unaged block.
fn create_age(b: Block) -> u8 {
    let mut age = 0;
    let mut at = b;
    while let Some(previous) = decor::copper_previous(at) {
        at = previous;
        age += 1;
    }
    age
}

/// Vanilla's weathering copper blocks, unaged (`copper_block` is `copper` once aged).
const VANILLA: [&str; 15] = [
    "copper",
    "cut_copper",
    "cut_copper_stairs",
    "cut_copper_slab",
    "chiseled_copper",
    "copper_door",
    "copper_trapdoor",
    "copper_grate",
    "copper_bulb",
    "copper_chest",
    "copper_golem_statue",
    "lightning_rod",
    "copper_bars",
    "copper_chain",
    "copper_lantern",
];

/// The weathering age of the block `id` is, when it is weathering copper (`instanceof
/// WeatheringCopper`): Create's from the engine, vanilla's from its id. No API names the class.
fn age_of(id: &str) -> Option<u8> {
    if let Some(name) = id.strip_prefix("create:") {
        let b = Block::from_name(name)?;
        return weathering(b).then(|| create_age(b));
    }
    let path = id.strip_prefix("minecraft:")?;
    if path == "copper_block" {
        return Some(0);
    }
    let (age, rest) = [("exposed_", 1), ("weathered_", 2), ("oxidized_", 3)]
        .into_iter()
        .find_map(|(p, a)| path.strip_prefix(p).map(|r| (a, r)))
        .unwrap_or((0, path));
    (VANILLA.contains(&rest) && !(age == 0 && rest == "copper")).then_some(age)
}

/// `state` as block `to`, its properties kept (`Block.withPropertiesOf`).
fn as_block(host: &dyn Host, state: BlockState, to: Block) -> Option<BlockState> {
    let text = host.block_state_string(state)?;
    let (_, props) = states::split(&text);
    super::state_with(host, to, &props)
}

/// `state` as block `to` with its properties, read off the level.
fn as_block_on(level: &dyn LevelHost, state: BlockState, to: Block) -> Option<BlockState> {
    let text = level.state_string(state)?;
    let (_, props) = states::split(&text);
    let base = level.block_state_default(states::key(to))?;
    props
        .iter()
        .try_fold(base, |s, (k, v)| level.state_with_property(s, k, v))
}

struct Copper;

#[mod_api::horizon_part]
impl mod_api::BlockPart for Copper {
    /// `ChangeOverTimeBlock.changeOverTime`: vanilla's chance and neighbour rule, into Create's
    /// next age. Vanilla's own random tick (which would look for vanilla's next copper) is
    /// skipped.
    fn random_tick(&self, level: &mut dyn LevelWriteHost, event: &mut RandomTickEvent) -> Flow {
        let Some(id) = level.state_block(event.state).map(str::to_owned) else {
            return Flow::Continue;
        };
        let Some(block) = id.strip_prefix("create:").and_then(Block::from_name) else {
            return Flow::Continue;
        };
        let Some(next) = decor::copper_next(block) else {
            return Flow::Continue;
        };
        let Some(mut random) = level.random() else {
            return Flow::Continue;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        let grown = change_over_time(&*level, &mut random, pos, create_age(block));
        level.set_random(random);
        if grown && let Some(to) = as_block_on(&*level, event.state, next) {
            level.set_block(pos, to, super::super::server::update_all());
        }
        Flow::Cancel
    }

    /// `AxeItem.evaluateNewBlockState`: scraping steps Create's copper back an age, wax-off
    /// takes its wax; the axe's sound, level event, wear and game event are the game's.
    fn tool_modified_state(&self, host: &dyn Host, event: &mut ToolModifiedStateEvent<'_>) -> Flow {
        let Some(block) = create_block(host, event.state) else {
            return Flow::Continue;
        };
        let to = match event.tool.ability {
            ToolAbility::AxeScrape => decor::copper_previous(block),
            ToolAbility::AxeWaxOff => decor::copper_unwaxed(block),
            _ => None,
        };
        if let Some(to) = to.and_then(|b| as_block(host, event.state, b)) {
            *event.final_state = Some(to);
        }
        Flow::Continue
    }
}

/// The chance draws of `changeOverTime` for a block of age `own` at `pos`: whether it ages.
fn change_over_time(
    level: &dyn LevelHost,
    random: &mut impl RandomSource,
    pos: [i32; 3],
    own: u8,
) -> bool {
    if random.next_float() >= 0.056_888_89 {
        return false;
    }
    let (mut same, mut older) = (0u32, 0u32);
    for x in -4i32..=4 {
        for y in -4i32..=4 {
            for z in -4i32..=4 {
                if x.abs() + y.abs() + z.abs() > 4 || (x, y, z) == (0, 0, 0) {
                    continue;
                }
                let at = [pos[0] + x, pos[1] + y, pos[2] + z];
                let Some(age) = level
                    .block_state(at)
                    .and_then(|s| level.state_block(s))
                    .and_then(age_of)
                else {
                    continue;
                };
                match age.cmp(&own) {
                    std::cmp::Ordering::Less => return false,
                    std::cmp::Ordering::Greater => older += 1,
                    std::cmp::Ordering::Equal => same += 1,
                }
            }
        }
    }
    let chance = (older + 1) as f32 / (older + same + 1) as f32;
    let modifier = if own == 0 { 0.75 } else { 1.0 };
    random.next_float() < chance * chance * modifier
}

/// Honeycomb on Create's copper: `HoneycombItem.useOn`.
struct Honeycomb;

#[mod_api::horizon_part]
impl ItemPart for Honeycomb {
    /// Into Create's waxed twin: the `item_used_on_block` trigger, one honeycomb used (not in
    /// creative), the block changed, the `block_change` game event and the wax-on level event
    /// (3003: its particles and sound).
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return None;
        };
        let player = host.player(event.player)?;
        let dimension = player_dimension(&*host, player)?;
        let mut lv = ServerLevel::new(host, &dimension)?;
        let pos = event.pos;
        let state = lv.block_state(pos)?;
        let block = create_block(&*lv.host, state)?;
        let waxed = as_block(&*lv.host, state, decor::copper_waxed(block)?)?;
        let _ = player.trigger_item_used_on_block(&mut *lv.host, pos, event.stack);
        use_one(&mut *lv.host, player, event.hand, event.stack);
        lv.put(pos, waxed);
        let context = GameEventContext {
            source_entity: Some(player.entity()),
            affected_state: Some(waxed),
        };
        lv.level.game_event(
            &mut *lv.host,
            "minecraft:block_change",
            super::center(pos),
            context,
        );
        lv.level.level_event(&mut *lv.host, 3003, pos, 0);
        Some(InteractionResult::Success)
    }
}

#[cfg(test)]
mod tests {
    use super::age_of;

    #[test]
    fn weathering_ages_of_create_and_vanilla_copper() {
        assert_eq!(age_of("create:copper_shingles"), Some(0));
        assert_eq!(age_of("create:weathered_copper_tile_stairs"), Some(2));
        assert_eq!(age_of("create:waxed_exposed_copper_shingles"), None);
        assert_eq!(age_of("minecraft:copper_block"), Some(0));
        assert_eq!(age_of("minecraft:oxidized_copper"), Some(3));
        assert_eq!(age_of("minecraft:exposed_cut_copper_slab"), Some(1));
        assert_eq!(age_of("minecraft:waxed_copper_block"), None);
        assert_eq!(age_of("minecraft:copper_ore"), None);
        assert_eq!(age_of("minecraft:stone"), None);
    }
}
