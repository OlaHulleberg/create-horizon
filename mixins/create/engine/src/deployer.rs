//! Create's deployer, first slice (`DeployerBlockEntity`, `BeltDeployerCallbacks`,
//! `DeployerApplicationRecipe`, `ItemApplicationRecipe`): it holds one stack, and facing down
//! over a depot whose item a `create:deploying` or `create:item_application` recipe takes with
//! the held item, it reaches down, applies it and draws back, using up the held item unless the
//! recipe keeps it. Its other uses (placing, clicking and punching as a fake player) are gap 6.

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

/// A deployer's typed state: its cycle, `timer` (1000 a stroke) and `reach`, and the stack in
/// its hand.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Deployer {
    pub state: State,
    pub timer: i32,
    pub reach: f32,
    pub held: Option<(String, u8)>,
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
                Tick::Changed
            }
            State::Retracting => {
                self.state = State::Waiting;
                self.timer = 500;
                Tick::Changed
            }
        }
    }

    /// Whether it ticks at all: a stroke under way or a wait running down.
    pub fn busy(&self) -> bool {
        self.state != State::Waiting || self.timer > 0
    }

    /// `getHandOffset` at the start of a tick, and blocks a tick it moves at: the hand runs out
    /// over the stroke to `reach + handLength` (4/16 holding, none pointing), at most 21/16.
    pub fn hand(&self, rpm: f32) -> (f32, f32) {
        let length = if self.held.is_some() { 4.0 / 16.0 } else { 0.0 };
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
/// deployer facing down; `fits` whether a recipe takes the stack with the held item.
pub fn on_depot(d: &Deployer, rpm: f32, locked: bool, fits: bool) -> Depot {
    if rpm == 0.0 {
        return Depot::Pass;
    }
    if !locked && d.state != State::Waiting {
        return Depot::Hold;
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
        return Depot::Start;
    }
    Depot::Hold
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
}
