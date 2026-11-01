// Stand-in for `mixins/create/equipment_body.rs` in the template test: the fixture's stacks have
// no components, so a backtank's air is kept in `damage`; the body is what the test sets.
use super::Game;
use create_engine::equipment::Diver;
use horizon_client::items::ItemStack;

thread_local! {
    /// (can breathe, in lava) of the acting player, as the test sets it.
    pub(crate) static BODY: std::cell::Cell<(bool, bool)> = const { std::cell::Cell::new((true, false)) };
    /// How many times the air supply was refilled.
    pub(crate) static REFILLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// The reach modifiers on the player.
    pub(crate) static REACH: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Hands worn (true: the off hand).
    pub(crate) static WORN: std::cell::RefCell<Vec<bool>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(super) fn stack_air(stack: &ItemStack) -> i32 {
    stack.damage as i32
}

pub(super) fn set_stack_air(stack: &mut ItemStack, air: i32) {
    stack.damage = air.max(0) as u32;
}

pub(super) fn capacity(stack: &ItemStack) -> i32 {
    let _ = stack;
    0
}

pub(super) fn set_capacity(stack: &mut ItemStack, level: i32) {
    let _ = (stack, level);
}

pub(super) struct Body {
    pub(super) can_breathe: bool,
    pub(super) in_lava: bool,
    pub(super) creative: bool,
}

pub(super) fn body(game: &Game<'_>) -> Option<Body> {
    let (can_breathe, in_lava) = BODY.get();
    Some(Body {
        can_breathe,
        in_lava,
        creative: game.creative,
    })
}

pub(super) fn refill_air(game: &mut Game<'_>) {
    let _ = game;
    REFILLS.set(REFILLS.get() + 1);
}

pub(super) fn has_reach(game: &mut Game<'_>, id: &str) -> bool {
    let _ = game;
    REACH.with_borrow(|r| r.iter().any(|m| m == id))
}

pub(super) fn set_reach(game: &mut Game<'_>, modifier: (&str, f64), on: bool) {
    let _ = game;
    REACH.with_borrow_mut(|r| {
        r.retain(|m| m != modifier.0);
        if on {
            r.push(modifier.0.to_owned());
        }
    });
}

pub(super) fn damage_hand(game: &mut Game<'_>, off: bool) {
    let _ = game;
    WORN.with_borrow_mut(|w| w.push(off));
}

pub(super) fn diver(game: &Game<'_>) -> Option<(Diver, bool, bool, bool)> {
    let _ = game;
    None
}

pub(super) fn set_dive(game: &mut Game<'_>, motion: [f64; 3], on_ground: bool) {
    let _ = (game, motion, on_ground);
}
