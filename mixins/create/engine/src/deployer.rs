//! Create's deployer, first slice (`DeployerBlockEntity`, `BeltDeployerCallbacks`,
//! `DeployerApplicationRecipe`, `ItemApplicationRecipe`): it holds one stack, and facing down
//! over a depot whose item a `create:deploying` or `create:item_application` recipe takes with
//! the held item, it reaches down, applies it and draws back, using up the held item unless the
//! recipe keeps it. Placing blocks and using items on blocks are in the glue (`deployer_activate`);
//! what the fake player (`DeployerFakePlayer`, `DeployerHandler.activateInner`) does is ruled
//! here: the mode it is in, where it reaches, the items it uses on the block there, and its
//! punch (`Mode.PUNCH`: attacking entities, breaking the block in steps).

use crate::kinetics::Dir;
use crate::processing::Output;

/// `DeployerBlockEntity.State`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Waiting,
    Expanding,
    Retracting,
}

impl State {
    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(i: u8) -> State {
        match i {
            1 => State::Expanding,
            2 => State::Retracting,
            _ => State::Waiting,
        }
    }
}

/// `DeployerBlockEntity.Mode`: using the held item, or punching with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Use,
    Punch,
}

impl Mode {
    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(i: u8) -> Mode {
        if i == 1 { Mode::Punch } else { Mode::Use }
    }

    /// `changeMode`.
    pub fn toggled(self) -> Mode {
        match self {
            Mode::Use => Mode::Punch,
            Mode::Punch => Mode::Use,
        }
    }
}

/// A deployer's typed state: its cycle, `timer` (1000 a stroke) and `reach`, the stack in its
/// hand, its mode, and the fake player's `blockBreakingProgress` (not saved, as in Create).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Deployer {
    pub state: State,
    pub timer: i32,
    pub reach: f32,
    pub held: Option<(String, u8)>,
    pub mode: Mode,
    pub breaking: Option<([i32; 3], f32)>,
    /// `redstoneLocked`: powered, it starts no stroke (saved, as `Powered`).
    pub locked: bool,
}

/// `getTimerSpeed`: twice the speed, held between 8 and 512 (0 when still).
pub fn timer_speed(rpm: f32) -> i32 {
    if rpm == 0.0 {
        0
    } else {
        (rpm * 2.0).abs().clamp(8.0, 512.0) as i32
    }
}

/// What a tick did that clients must see (`sendData`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    None,
    Changed,
    /// At the bottom of the stroke (`activate`): it draws back from here.
    Activate,
}

impl Deployer {
    /// `start`: reach down, `reach` from the ray toward the block two ahead (`.5 + min(hit,
    /// .75)`).
    pub fn start(&mut self, hit: f32) {
        self.state = State::Expanding;
        self.reach = 0.5 + hit.min(0.75);
        self.timer = 1000;
    }

    /// `tick` for a deployer over a depot (no fake player): the stroke runs down; at the bottom
    /// (`activate` hands the work to the depot) it draws back, then waits 500.
    pub fn tick(&mut self, rpm: f32) -> Tick {
        let speed = timer_speed(rpm);
        if speed == 0 {
            return Tick::None;
        }
        if self.timer > 0 {
            self.timer -= speed;
            return Tick::None;
        }
        match self.state {
            State::Waiting => Tick::None,
            State::Expanding => {
                self.state = State::Retracting;
                self.timer = 1000;
                Tick::Activate
            }
            State::Retracting => {
                self.state = State::Waiting;
                self.timer = 500;
                Tick::Changed
            }
        }
    }

    /// `timer = getTimerSpeed() * 10`: nothing to do yet, look again later.
    pub fn wait(&mut self, rpm: f32) {
        self.timer = timer_speed(rpm) * 10;
    }

    /// Whether it ticks at all: a stroke under way or a wait running down.
    pub fn busy(&self) -> bool {
        self.state != State::Waiting || self.timer > 0
    }

    /// `getHandOffset` at the start of a tick, and blocks a tick it moves at: the hand runs out
    /// over the stroke to `reach + handLength` (3/16 punching, else 4/16 holding, none
    /// pointing), at most 21/16.
    pub fn hand(&self, rpm: f32) -> (f32, f32) {
        let length = if self.mode == Mode::Punch {
            3.0 / 16.0
        } else if self.held.is_some() {
            4.0 / 16.0
        } else {
            0.0
        };
        let full = self.reach + length;
        let speed = timer_speed(rpm) as f32 / 1000.0;
        let progress = match self.state {
            State::Expanding => 1.0 - self.timer as f32 / 1000.0,
            State::Retracting => self.timer as f32 / 1000.0,
            State::Waiting => 0.0,
        };
        let at = (progress.clamp(0.0, 1.0) * full).min(21.0 / 16.0);
        let v = match self.state {
            State::Expanding if at < full.min(21.0 / 16.0) => speed * full,
            State::Retracting if at > 0.0 => -speed * full,
            _ => 0.0,
        };
        (at, v)
    }
}

/// A `create:deploying` or `create:item_application` recipe: the item worked on, the item held,
/// its results, and whether the held item is kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub target: Vec<String>,
    pub tool: Vec<String>,
    pub results: Vec<Output>,
    pub keep_held: bool,
}

/// `getRecipe`: the first recipe whose item is `target` and whose tool is `held`.
pub fn find<'a>(recipes: &'a [Recipe], target: &str, held: &str) -> Option<&'a Recipe> {
    recipes
        .iter()
        .find(|r| r.target.iter().any(|t| t == target) && r.tool.iter().any(|t| t == held))
}

/// What the deployer says about the depot's stack (`ProcessingResult`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depot {
    Pass,
    Hold,
    /// Hold, and the deployer starts its stroke.
    Start,
    /// Hold, and apply the recipe now.
    Apply,
}

/// `BeltDeployerCallbacks.onItemReceived` (`locked` false) and `whenItemHeld` (true) for a
/// deployer facing down; `fits` whether a recipe takes the stack with the held item. A punching
/// deployer lets new stacks pass.
pub fn on_depot(d: &Deployer, rpm: f32, locked: bool, fits: bool) -> Depot {
    if rpm == 0.0 {
        return Depot::Pass;
    }
    if !locked && d.mode == Mode::Punch {
        return Depot::Pass;
    }
    if !locked && d.state != State::Waiting {
        return Depot::Hold;
    }
    if !locked && d.locked {
        return Depot::Pass;
    }
    if d.held.is_none() {
        return Depot::Hold;
    }
    if !fits {
        return Depot::Pass;
    }
    if locked && d.state == State::Retracting && d.timer == 1000 {
        return Depot::Apply;
    }
    if d.state == State::Waiting {
        // `whenItemHeld`: a locked one lets it go.
        if d.locked {
            return Depot::Pass;
        }
        return Depot::Start;
    }
    Depot::Hold
}

/// Where a block the deployer holds goes (`BlockItem.useOn` from `DeployerHandler`): the block
/// it reaches (two ahead) when that can be replaced, else the space before it; None when
/// neither can, or the block there already is the held one (`shouldActivate`).
pub fn placement(
    reached_replaceable: bool,
    before_replaceable: bool,
    reached_is_held: bool,
) -> Option<bool> {
    if reached_is_held {
        return None;
    }
    if reached_replaceable {
        Some(true)
    } else if before_replaceable {
        Some(false)
    } else {
        None
    }
}

/// The held stack after a recipe used it (`keepHeldItem`, else one less; damage is not
/// tracked yet).
pub fn use_held(held: &mut Option<(String, u8)>, recipe: &Recipe) {
    if recipe.keep_held {
        return;
    }
    if let Some((_, n)) = held.as_mut() {
        *n -= 1;
        if *n == 0 {
            *held = None;
        }
    }
}

/// The block the fake player reaches (`activate`'s `clickedPos`): two ahead of the deployer.
pub fn clicked(pos: [i32; 3], facing: Dir) -> [i32; 3] {
    let o = facing.offset();
    [pos[0] + o[0] * 2, pos[1] + o[1] * 2, pos[2] + o[2] * 2]
}

/// `activateInner`'s ray, from the deployer's centre `3/2 + 1/64` to `5/2 - 1/64` along its
/// facing; the fake player stands at its start, eyes at its feet (`deployerHasEyesOnHisFeet`).
pub fn ray(pos: [i32; 3], facing: Dir) -> ([f64; 3], [f64; 3]) {
    let o = facing.offset();
    let at = |scale: f64| -> [f64; 3] {
        std::array::from_fn(|i| f64::from(pos[i]) + 0.5 + f64::from(o[i]) * scale)
    };
    (at(3.0 / 2.0 + 1.0 / 64.0), at(5.0 / 2.0 - 1.0 / 64.0))
}

/// The face the fake player clicks: the ray starts and ends inside the block it reaches, so
/// the hit (or the miss, `Direction.getNearest(from - to)`) faces back toward the deployer.
pub fn clicked_face(facing: Dir) -> Dir {
    facing.opposite()
}

/// What `ItemStack.useOn` / `Item.use` does with a held item that is not a block, for the items
/// the port knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    /// An axe, hoe or shovel (`minecraft:block_transformer`): strips, scrapes, unwaxes, tills or
    /// flattens the block it reaches (`BlockTransformer.transformBlock`).
    Transform,
    /// `SpawnEggItem.useOn`.
    SpawnEgg,
    /// An item whose use acts on the block the way its dispenser behaviour does (buckets,
    /// flint and steel, bone meal, glass bottles, glowstone, shears, honeycomb, brushes).
    Dispense,
    Nothing,
}

/// The items that act on the block the way their dispenser behaviour does.
pub const DISPENSED: [&str; 11] = [
    "minecraft:bucket",
    "minecraft:water_bucket",
    "minecraft:lava_bucket",
    "minecraft:powder_snow_bucket",
    "minecraft:flint_and_steel",
    "minecraft:bone_meal",
    "minecraft:glass_bottle",
    "minecraft:glowstone",
    "minecraft:shears",
    "minecraft:honeycomb",
    "minecraft:brush",
];

/// Which use `item` has; `transformer` whether it has a `minecraft:block_transformer`.
pub fn item_use(item: &str, transformer: bool) -> Use {
    if transformer {
        Use::Transform
    } else if item.ends_with("_spawn_egg") {
        Use::SpawnEgg
    } else if DISPENSED.contains(&item) {
        Use::Dispense
    } else {
        Use::Nothing
    }
}

/// `SpawnEggItem.useOn` from the deployer (`clicked_face`): the mob comes at the block it
/// reaches when that has no collision shape, else in the space before it.
pub fn spawn_egg_at(pos: [i32; 3], facing: Dir, reached_empty: bool) -> [i32; 3] {
    let reached = clicked(pos, facing);
    if reached_empty {
        return reached;
    }
    let o = clicked_face(facing).offset();
    [reached[0] + o[0], reached[1] + o[1], reached[2] + o[2]]
}

/// `SpawnEggItem.useOn`'s `shouldOffsetYMore` for `EntityType.spawn`: the mob comes before the
/// block it reaches (not into it) and the face clicked is the top, as for a deployer facing down
/// onto a block with a collision shape.
pub fn spawn_egg_offsets_more(facing: Dir, reached_empty: bool) -> bool {
    !reached_empty && clicked_face(facing) == Dir::Up
}

/// What `getDestroyProgress` reads of the block and the held tool: its hardness (-1
/// unbreakable), the tool's `Tool.getMiningSpeed` and `mining_efficiency` bonus, and whether it
/// harvests it (`!requiresCorrectToolForDrops || isCorrectForDrops`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dig {
    pub hardness: f32,
    pub tool_speed: f32,
    pub efficiency: f32,
    pub harvest: bool,
}

/// `BlockBehaviour.getDestroyProgress` for the fake player (`Player.getDestroySpeed`): the tool's
/// speed, plus its efficiency above 1; no effects (`canBeAffected` is false), the block break
/// speed attribute at 1; never eyes in water nor on the ground (a fake player never ticks), so a
/// fifth; over the hardness and 30 when it harvests, else 100. 0 for an unbreakable block.
pub fn destroy_progress(d: Dig) -> f32 {
    if d.hardness == -1.0 {
        return 0.0;
    }
    let mut speed = d.tool_speed;
    if speed > 1.0 {
        speed += d.efficiency;
    }
    speed /= 5.0;
    speed / d.hardness / if d.harvest { 30.0 } else { 100.0 }
}

/// What a punch at a block did (`activateInner`, `Mode.PUNCH`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Punch {
    /// The block breaks now (`tryHarvestBlock`); its cracks go.
    Broken,
    /// No progress: the breaking is forgotten.
    Stop,
    /// Progress kept; the cracks move to this stage when it changed (`destroyBlockProgress`).
    Crack(Option<i32>),
}

impl Deployer {
    /// A punch with a held stack at the block at `pos` whose `destroy_progress` is `per_hit`:
    /// sixteen times that, added to the progress so far.
    pub fn punch(&mut self, pos: [i32; 3], per_hit: f32) -> Punch {
        let before = self.breaking.map_or(0.0, |(_, p)| p);
        let progress = per_hit * 16.0 + before;
        if progress >= 1.0 {
            self.breaking = None;
            return Punch::Broken;
        }
        if progress <= 0.0 {
            self.breaking = None;
            return Punch::Stop;
        }
        let stage = progress * 10.0;
        let moved = (before * 10.0) as i32 != stage as i32;
        self.breaking = Some((pos, progress));
        Punch::Crack(moved.then_some(stage as i32))
    }
}

/// `getCurrentItemAttackStrengthDelay`: 1/64 tick for the fake player.
pub const ATTACK_DELAY: f32 = 1.0 / 64.0;

/// `Player.getAttackStrengthScale(.5)` right after `resetAttackStrengthTicker`.
pub fn attack_strength() -> f32 {
    (0.5 / ATTACK_DELAY).clamp(0.0, 1.0)
}

/// `Player.attack`'s damage: the base 1 and the held item's `attack_damage` modifier
/// (`activate` adds its modifiers), scaled by the attack strength (`0.2 + s² × 0.8`); never a
/// critical hit (it never falls). Enchantments are not counted (held stacks carry none here).
pub fn attack_damage(item_bonus: f32) -> f32 {
    let s = attack_strength();
    (1.0 + item_bonus) * (0.2 + s * s * 0.8)
}

/// The breaker id a deployer's cracks go by (Create: its fake player's entity id): negative, so
/// it never meets a real entity's.
pub fn breaker_id(pos: [i32; 3]) -> i32 {
    let h = pos[0].wrapping_mul(73_856_093)
        ^ pos[1].wrapping_mul(19_349_663)
        ^ pos[2].wrapping_mul(83_492_791);
    h | i32::MIN
}

/// The mod message channel for deployers' strokes, server to client.
pub const CHANNEL: &str = "create:deployer";

/// A deployer's stroke as clients get it: the hand's offset along `facing` now and blocks a
/// tick it moves, what it holds; `at` NaN when the deployer is gone.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub pos: [i32; 3],
    pub facing: Dir,
    pub at: f32,
    pub per_tick: f32,
    pub held: Option<String>,
}

pub fn encode(list: &[Shown]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in list {
        for v in s.pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(s.facing as u8);
        out.extend_from_slice(&s.at.to_le_bytes());
        out.extend_from_slice(&s.per_tick.to_le_bytes());
        let item = s.held.as_deref().unwrap_or("");
        let b = &item.as_bytes()[..item.len().min(255)];
        out.push(b.len() as u8);
        out.extend_from_slice(b);
    }
    out
}

pub fn decode(mut data: &[u8]) -> Vec<Shown> {
    let mut out = Vec::new();
    while data.len() >= 22 {
        let int = |i: usize| i32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let float = |i: usize| f32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let Some(&facing) = Dir::ALL.get(usize::from(data[12])) else {
            break;
        };
        let n = usize::from(data[21]);
        let Some(item) = data.get(22..22 + n) else {
            break;
        };
        let item = String::from_utf8_lossy(item).into_owned();
        out.push(Shown {
            pos: [int(0), int(4), int(8)],
            facing,
            at: float(13),
            per_tick: float(17),
            held: (!item.is_empty()).then_some(item),
        });
        data = &data[22 + n..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cog() -> Recipe {
        Recipe {
            id: "create:deploying/cogwheel".into(),
            target: vec!["create:shaft".into()],
            tool: vec!["minecraft:oak_planks".into()],
            results: vec![Output {
                item: "create:cogwheel".into(),
                count: 1,
                chance: 1.0,
            }],
            keep_held: false,
        }
    }

    #[test]
    fn a_stroke_over_a_depot_applies_once_at_the_bottom() {
        let mut d = Deployer {
            held: Some(("minecraft:oak_planks".into(), 2)),
            ..Deployer::default()
        };
        let r = [cog()];
        let fits = find(&r, "create:shaft", "minecraft:oak_planks").is_some();
        assert!(fits && find(&r, "create:shaft", "minecraft:stone").is_none());
        // The stack arrives: the deployer starts (16 rpm: 32 a tick).
        assert_eq!(on_depot(&d, 16.0, false, fits), Depot::Start);
        d.start(0.25);
        assert_eq!(d.reach, 0.75);
        let mut ticks = 0;
        let mut applied = None;
        for t in 0..200 {
            if on_depot(&d, 16.0, true, fits) == Depot::Apply {
                applied = Some(t);
                use_held(&mut d.held, &r[0]);
            }
            d.tick(16.0);
            ticks += 1;
            if applied.is_some() && d.state == State::Waiting {
                break;
            }
        }
        // 1000 / 32: 32 ticks down, then the next tick applies.
        assert_eq!(applied, Some(33));
        assert_eq!(d.held, Some(("minecraft:oak_planks".into(), 1)));
        assert!(ticks > 60, "{ticks}");
        assert_eq!(timer_speed(1.0), 8);
        assert_eq!(timer_speed(-300.0), 512);
    }

    #[test]
    fn strokes_round_trip() {
        let list = vec![
            Shown {
                pos: [1, -2, 3],
                facing: Dir::Down,
                at: 0.5,
                per_tick: -0.032,
                held: Some("minecraft:oak_planks".into()),
            },
            Shown {
                pos: [4, 5, 6],
                facing: Dir::East,
                at: 0.0,
                per_tick: 0.0,
                held: None,
            },
        ];
        assert_eq!(decode(&encode(&list)), list);
    }

    #[test]
    fn the_fake_player_reaches_into_the_block_two_ahead() {
        for facing in Dir::ALL {
            let pos = [3, -4, 5];
            let at = clicked(pos, facing);
            let (from, to) = ray(pos, facing);
            // Both ends of the ray lie inside the block it reaches, 1/64 from its faces.
            for end in [from, to] {
                assert_eq!(end.map(|v| v.floor() as i32), at, "{facing:?}");
            }
            assert_eq!(clicked_face(facing).offset().map(|v| -v), facing.offset());
        }
        let (from, to) = ray([0, 0, 0], Dir::East);
        assert_eq!((from[0], to[0]), (2.015625, 2.984375));
        // A spawn egg: in the block reached when it is passable, else the space before it.
        assert_eq!(spawn_egg_at([0, 0, 0], Dir::Down, true), [0, -2, 0]);
        assert_eq!(spawn_egg_at([0, 0, 0], Dir::Down, false), [0, -1, 0]);
        // Over a block with a shape, a deployer facing down offsets it more; nothing else does.
        assert!(spawn_egg_offsets_more(Dir::Down, false));
        assert!(!spawn_egg_offsets_more(Dir::Down, true));
        for facing in [Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East] {
            assert!(!spawn_egg_offsets_more(facing, false), "{facing:?}");
        }
    }

    #[test]
    fn items_used_on_the_block() {
        assert_eq!(item_use("minecraft:iron_hoe", true), Use::Transform);
        assert_eq!(item_use("minecraft:pig_spawn_egg", false), Use::SpawnEgg);
        assert_eq!(item_use("minecraft:water_bucket", false), Use::Dispense);
        assert_eq!(item_use("minecraft:stick", false), Use::Nothing);
    }

    #[test]
    fn punching_breaks_in_strokes() {
        let mut d = Deployer {
            mode: Mode::Punch,
            ..Deployer::default()
        };
        let at = [0, 0, 0];
        // A diamond pickaxe (8) on stone (1.5): 8 / 5 / 1.5 / 30 × 16 = .569 a stroke.
        let diamond = destroy_progress(Dig {
            hardness: 1.5,
            tool_speed: 8.0,
            efficiency: 0.0,
            harvest: true,
        });
        assert!((diamond * 16.0 - 0.568_888).abs() < 1e-5, "{diamond}");
        assert_eq!(d.punch(at, diamond), Punch::Crack(Some(5)));
        assert_eq!(d.punch(at, diamond), Punch::Broken);
        assert_eq!(d.breaking, None);
        // A wooden pickaxe (2): .142 a stroke, eight strokes.
        let wood = destroy_progress(Dig {
            hardness: 1.5,
            tool_speed: 2.0,
            efficiency: 0.0,
            harvest: true,
        });
        let strokes = (1..20).find(|_| d.punch(at, wood) == Punch::Broken);
        assert_eq!(strokes, Some(8));
        // A stick on stone it cannot harvest (1, over 100): 1 / 5 / 1.5 / 100 × 16.
        let stick = destroy_progress(Dig {
            hardness: 1.5,
            tool_speed: 1.0,
            efficiency: 5.0,
            harvest: false,
        });
        assert!((stick * 16.0 - 0.021_333).abs() < 1e-5, "{stick}");
        // Bedrock: no progress, and none is kept.
        let bedrock = Dig {
            hardness: -1.0,
            tool_speed: 8.0,
            efficiency: 0.0,
            harvest: false,
        };
        assert_eq!(d.punch(at, destroy_progress(bedrock)), Punch::Stop);
        // Instant blocks (0 hardness) break on the first stroke.
        let grass = Dig {
            hardness: 0.0,
            ..bedrock
        };
        assert_eq!(d.punch(at, destroy_progress(grass)), Punch::Broken);
    }

    #[test]
    fn punching_hits_at_full_strength() {
        assert_eq!(attack_strength(), 1.0);
        assert_eq!(attack_damage(0.0), 1.0);
        // A diamond sword's +6.
        assert_eq!(attack_damage(6.0), 7.0);
        let ids = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]].map(breaker_id);
        assert!(ids.iter().all(|i| *i < 0));
        assert!((1..4).all(|i| !ids[..i].contains(&ids[i])));
    }

    #[test]
    fn a_punching_deployer_lets_stacks_pass() {
        let mut d = Deployer {
            held: Some(("minecraft:oak_planks".into(), 1)),
            mode: Mode::Use.toggled(),
            ..Deployer::default()
        };
        assert_eq!(d.mode, Mode::Punch);
        assert_eq!(Mode::from_index(d.mode.index()), Mode::Punch);
        assert_eq!(on_depot(&d, 16.0, false, true), Depot::Pass);
        d.mode = d.mode.toggled();
        assert_eq!(on_depot(&d, 16.0, false, true), Depot::Start);
        // The punching hand is 3/16 long.
        d.mode = Mode::Punch;
        d.start(0.25);
        d.timer = 0;
        assert!((d.hand(16.0).0 - (0.75 + 3.0 / 16.0)).abs() < 1e-6);
    }

    #[test]
    fn the_hand_runs_out_to_its_reach() {
        let mut d = Deployer {
            held: Some(("x".into(), 1)),
            ..Deployer::default()
        };
        d.start(0.25);
        assert_eq!(d.hand(16.0), (0.0, 0.032));
        d.timer = 0;
        let (at, v) = d.hand(16.0);
        assert!((at - 1.0).abs() < 1e-6 && v == 0.0);
    }

    #[test]
    fn a_powered_deployer_lets_stacks_pass() {
        let d = Deployer {
            held: Some(("minecraft:oak_planks".into(), 2)),
            locked: true,
            ..Deployer::default()
        };
        assert_eq!(on_depot(&d, 16.0, false, true), Depot::Pass);
        assert_eq!(on_depot(&d, 16.0, true, true), Depot::Pass);
        let free = Deployer { locked: false, ..d };
        assert_eq!(on_depot(&free, 16.0, false, true), Depot::Start);
    }
}
