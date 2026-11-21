//! `GhostItemMenu`, `HeldItemGhostItemMenu`, `AbstractFilterMenu`, `FilterMenu` and
//! `AttributeFilterMenu`'s clicks, over `AbstractContainerMenu.doClick` for the player's slots.
//!
//! Slots are numbered as Create's menus number them: the player's 36 first (`addPlayerSlots`:
//! the three rows, then the hotbar), then the ghost slots.

/// A stack as the menu moves it.
pub trait Stack: Clone {
    fn count(&self) -> u32;
    fn with_count(&self, count: u32) -> Self;
    /// `getMaxStackSize`.
    fn max(&self) -> u32;
    /// `ItemStack.isSameItemSameComponents`.
    fn same(&self, other: &Self) -> bool;
}

/// The player's slots before the ghost slots.
pub const PLAYER_SLOTS: usize = 36;
/// `AbstractContainerMenu.SLOT_CLICKED_OUTSIDE`.
pub const OUTSIDE: i32 = -999;
/// `Inventory.SLOT_OFFHAND`, as a swap's button.
pub const OFFHAND: i32 = 40;

/// Which of Create's ghost item menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Filter,
    AttributeFilter,
}

impl Kind {
    /// `createGhostInventory`'s size.
    pub fn ghost_slots(self) -> usize {
        match self {
            Kind::Filter => crate::filter::LIST_SLOTS,
            Kind::AttributeFilter => 2,
        }
    }

    /// `AttributeFilterMenu`'s selected attributes' slot (index 37), which nothing changes.
    fn fixed(self, ghost: usize) -> bool {
        self == Kind::AttributeFilter && ghost == 1
    }
}

/// `ClickType`, by its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Pickup,
    QuickMove,
    Swap,
    Clone,
    Throw,
    QuickCraft,
    PickupAll,
}

impl Click {
    pub fn of(mode: i32) -> Option<Click> {
        Some(match mode {
            0 => Click::Pickup,
            1 => Click::QuickMove,
            2 => Click::Swap,
            3 => Click::Clone,
            4 => Click::Throw,
            5 => Click::QuickCraft,
            6 => Click::PickupAll,
            _ => return None,
        })
    }
}

/// A drag in progress (`quickcraftStatus`, `quickcraftType`, `quickcraftSlots`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Drag {
    status: i32,
    kind: i32,
    slots: Vec<usize>,
}

/// An open ghost item menu over the held stack, and the player's inventory it shows.
#[derive(Clone, Debug)]
pub struct Menu<S> {
    pub kind: Kind,
    pub ghost: Vec<Option<S>>,
    /// The player's 36 inventory slots, the hotbar's 0..9 first.
    pub inventory: Vec<Option<S>>,
    pub offhand: Option<S>,
    pub carried: Option<S>,
    pub selected: usize,
    /// `player.hasInfiniteMaterials`.
    pub creative: bool,
    /// The inventory slot holding the stack the menu was opened for (`contentHolder`): 0..36,
    /// or 40 for the offhand; none once it left the inventory.
    pub holder: Option<usize>,
    drag: Drag,
}

/// A stack's place in the menu: a player slot by inventory index, or a ghost slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    Player(usize),
    Ghost(usize),
}

/// The inventory index of the player's menu slot `index` (`addPlayerSlots`).
pub fn inventory_index(index: usize) -> usize {
    if index < 27 { index + 9 } else { index - 27 }
}

impl<S: Stack> Menu<S> {
    pub fn new(
        kind: Kind,
        ghost: Vec<Option<S>>,
        inventory: Vec<Option<S>>,
        selected: usize,
    ) -> Menu<S> {
        let mut ghost = ghost;
        ghost.resize(kind.ghost_slots(), None);
        let mut inventory = inventory;
        inventory.resize(PLAYER_SLOTS, None);
        Menu {
            kind,
            ghost,
            inventory,
            offhand: None,
            carried: None,
            selected,
            creative: false,
            holder: Some(selected),
            drag: Drag::default(),
        }
    }

    /// `HeldItemGhostItemMenu.stillValid`: the selected slot still holds the stack.
    pub fn still_valid(&self) -> bool {
        self.holder == Some(self.selected)
    }

    fn at(index: usize) -> At {
        if index < PLAYER_SLOTS {
            At::Player(inventory_index(index))
        } else {
            At::Ghost(index - PLAYER_SLOTS)
        }
    }

    fn get(&self, at: At) -> Option<&S> {
        match at {
            At::Player(i) => self.inventory.get(i)?.as_ref(),
            At::Ghost(i) => self.ghost.get(i)?.as_ref(),
        }
    }

    fn set(&mut self, at: At, stack: Option<S>) {
        let stack = stack.filter(|s| s.count() > 0);
        match at {
            At::Player(i) => self.inventory[i] = stack,
            At::Ghost(i) => self.ghost[i] = stack,
        }
    }

    fn set_carried(&mut self, stack: Option<S>) {
        self.carried = stack.filter(|s| s.count() > 0);
    }

    fn slots(&self) -> usize {
        PLAYER_SLOTS + self.ghost.len()
    }

    /// `HeldItemGhostItemMenu.isInSlot`: the hotbar slot holding the stack the menu is of.
    fn held_slot(&self, index: usize) -> bool {
        index >= 27 && index - 27 == self.selected
    }

    /// `AbstractContainerMenu.clicked` with the menus' overrides: what the player's click on
    /// slot `index` (`OUTSIDE` or negative off the slots) does. Returns the stacks it throws.
    pub fn click(&mut self, index: i32, button: i32, click: Click) -> Vec<S> {
        let mut thrown = Vec::new();
        let slot = usize::try_from(index).ok().filter(|&i| i < self.slots());
        if index >= 0 && slot.is_none() {
            return thrown;
        }
        if let Some(i) = slot {
            // `AttributeFilterMenu.clicked`
            if i >= PLAYER_SLOTS && self.kind.fixed(i - PLAYER_SLOTS) {
                return thrown;
            }
            // `HeldItemGhostItemMenu.clicked`
            if self.held_slot(i) && !matches!(click, Click::Throw | Click::Clone) {
                return thrown;
            }
            // `GhostItemMenu.clicked`
            if i >= PLAYER_SLOTS {
                self.click_ghost(i - PLAYER_SLOTS, click);
                return thrown;
            }
        }
        self.do_click(index, slot, button, click, &mut thrown);
        thrown
    }

    fn click_ghost(&mut self, ghost: usize, click: Click) {
        match click {
            Click::Throw => {}
            Click::Clone => {
                if self.creative
                    && self.carried.is_none()
                    && let Some(s) = &self.ghost[ghost]
                {
                    self.carried = Some(s.with_count(s.max()));
                }
            }
            _ => self.ghost[ghost] = self.carried.as_ref().map(|s| s.with_count(1)),
        }
    }

    /// `AbstractContainerMenu.doClick` on the player's slots (or outside).
    fn do_click(
        &mut self,
        index: i32,
        slot: Option<usize>,
        button: i32,
        click: Click,
        thrown: &mut Vec<S>,
    ) {
        if click == Click::QuickCraft {
            self.quick_craft(slot, button);
            return;
        }
        if self.drag.status != 0 {
            self.drag = Drag::default();
            return;
        }
        match click {
            Click::Pickup | Click::QuickMove if button == 0 || button == 1 => {
                let primary = button == 0;
                if index == OUTSIDE {
                    if let Some(c) = self.carried.take() {
                        if primary {
                            thrown.push(c);
                        } else {
                            thrown.push(c.with_count(1));
                            self.set_carried(Some(c.with_count(c.count() - 1)));
                        }
                    }
                } else if let Some(i) = slot {
                    if click == Click::QuickMove {
                        self.quick_move(i);
                    } else {
                        self.pickup(At::Player(inventory_index(i)), primary);
                    }
                }
            }
            Click::Swap if (0..9).contains(&button) || button == OFFHAND => {
                if let Some(i) = slot {
                    self.swap(inventory_index(i), button, thrown);
                }
            }
            Click::Clone => {
                if self.creative
                    && self.carried.is_none()
                    && let Some(s) = slot.and_then(|i| self.get(Self::at(i)))
                {
                    self.carried = Some(s.with_count(s.max()));
                }
            }
            Click::Throw => {
                if self.carried.is_none()
                    && let Some(i) = slot
                {
                    let at = At::Player(inventory_index(i));
                    if let Some(s) = self.get(at).cloned() {
                        let n = if button == 0 { 1 } else { s.count() };
                        thrown.push(s.with_count(n));
                        self.set(at, Some(s.with_count(s.count() - n)));
                        if self.get(at).is_none() && self.holder == Some(inventory_index(i)) {
                            self.holder = None;
                        }
                    }
                }
            }
            Click::PickupAll => {
                if let Some(i) = slot {
                    self.pickup_all(i, button);
                }
            }
            _ => {}
        }
    }

    /// The `PICKUP` click on a player slot (`safeInsert`, `tryRemove`, `setByPlayer`).
    fn pickup(&mut self, at: At, primary: bool) {
        let here = self.get(at).cloned();
        let carried = self.carried.clone();
        match (here, carried) {
            (None, None) => {}
            (None, Some(c)) => {
                let n = if primary { c.count() } else { 1 };
                self.safe_insert(at, c, n);
            }
            (Some(h), None) => {
                let n = if primary {
                    h.count()
                } else {
                    h.count().div_ceil(2)
                };
                self.set_carried(Some(h.with_count(n)));
                self.set(at, Some(h.with_count(h.count() - n)));
            }
            (Some(h), Some(c)) => {
                if h.same(&c) {
                    let n = if primary { c.count() } else { 1 };
                    self.safe_insert(at, c, n);
                } else if c.count() <= c.max() {
                    self.set_carried(Some(h));
                    self.set(at, Some(c));
                }
            }
        }
    }

    /// `Slot.safeInsert`: up to `n` of the carried `stack` into `at`; what is left is carried.
    fn safe_insert(&mut self, at: At, stack: S, n: u32) {
        let here = self.get(at).cloned();
        let have = here.as_ref().map_or(0, Stack::count);
        let put = n.min(stack.count()).min(stack.max().saturating_sub(have));
        if put == 0 || here.as_ref().is_some_and(|h| !h.same(&stack)) {
            self.set_carried(Some(stack));
            return;
        }
        self.set(at, Some(stack.with_count(have + put)));
        self.set_carried(Some(stack.with_count(stack.count() - put)));
    }

    /// `quickMoveStack`: the shift-click puts a ghost copy of the slot's stack into the menu.
    fn quick_move(&mut self, index: usize) {
        let insert = self.get(Self::at(index)).cloned();
        match self.kind {
            Kind::AttributeFilter => self.ghost[0] = insert.map(|s| s.with_count(1)),
            Kind::Filter => {
                let Some(insert) = insert else { return };
                for g in self.ghost.iter_mut() {
                    match g {
                        Some(there) if there.same(&insert) => break,
                        Some(_) => {}
                        None => {
                            *g = Some(insert.with_count(1));
                            break;
                        }
                    }
                }
            }
        }
    }

    /// The `SWAP` click: inventory slot `slot` against hotbar slot `button` or the offhand.
    fn swap(&mut self, slot: usize, button: i32, thrown: &mut Vec<S>) {
        let other = if button == OFFHAND {
            None
        } else {
            Some(button as usize)
        };
        let theirs = match other {
            Some(b) => self.inventory[b].clone(),
            None => self.offhand.clone(),
        };
        let here = self.inventory[slot].clone();
        let put_other = |m: &mut Menu<S>, s: Option<S>| match other {
            Some(b) => m.inventory[b] = s,
            None => m.offhand = s,
        };
        match (theirs, here) {
            (None, None) => {}
            (None, Some(h)) => {
                put_other(self, Some(h));
                self.inventory[slot] = None;
            }
            (Some(t), None) => {
                if t.count() > t.max() {
                    self.inventory[slot] = Some(t.with_count(t.max()));
                    put_other(self, Some(t.with_count(t.count() - t.max())));
                } else {
                    put_other(self, None);
                    self.inventory[slot] = Some(t);
                }
            }
            (Some(t), Some(h)) => {
                if t.count() > t.max() {
                    self.inventory[slot] = Some(t.with_count(t.max()));
                    put_other(self, Some(t.with_count(t.count() - t.max())));
                    if let Some(rest) = self.add(h) {
                        thrown.push(rest);
                    }
                } else {
                    put_other(self, Some(h));
                    self.inventory[slot] = Some(t);
                }
            }
        }
        let other = other.unwrap_or(OFFHAND as usize);
        if self.holder == Some(other) {
            self.holder = Some(slot);
        } else if self.holder == Some(slot) {
            self.holder = Some(other);
        }
    }

    /// `Inventory.add`: onto a matching stack (the selected slot, the offhand, then the rest),
    /// then into an empty slot; what is left.
    fn add(&mut self, stack: S) -> Option<S> {
        let mut left = stack;
        let others = (0..PLAYER_SLOTS).filter(|&i| i != self.selected);
        let order: Vec<usize> = [self.selected, OFFHAND as usize]
            .into_iter()
            .chain(others)
            .collect();
        for i in order {
            let place = if i == OFFHAND as usize {
                &mut self.offhand
            } else {
                &mut self.inventory[i]
            };
            if let Some(s) = place.as_ref()
                && s.same(&left)
                && s.count() < s.max()
            {
                let n = (s.max() - s.count()).min(left.count());
                *place = Some(s.with_count(s.count() + n));
                left = left.with_count(left.count() - n);
                if left.count() == 0 {
                    return None;
                }
            }
        }
        if let Some(i) = self.inventory.iter().position(Option::is_none) {
            self.inventory[i] = Some(left);
            return None;
        }
        Some(left)
    }

    /// `canItemQuickReplace(slot, stack, true)`.
    fn quick_replace(&self, at: At, stack: &S) -> bool {
        match self.get(at) {
            None => true,
            Some(h) => h.same(stack) && h.count() <= stack.max(),
        }
    }

    /// `GhostItemMenu.canDragTo` (no repeats in a filter): the player's slots only.
    fn can_drag_to(index: usize) -> bool {
        index < PLAYER_SLOTS
    }

    /// The `QUICK_CRAFT` click: a drag's start, a slot it passes and its end.
    fn quick_craft(&mut self, slot: Option<usize>, button: i32) {
        let was = self.drag.status;
        self.drag.status = button & 3;
        if (was != 1 || self.drag.status != 2) && was != self.drag.status {
            self.drag = Drag::default();
            return;
        }
        let Some(carried) = self.carried.clone() else {
            self.drag = Drag::default();
            return;
        };
        match self.drag.status {
            0 => {
                self.drag.kind = (button >> 2) & 3;
                if self.drag.kind == 0
                    || self.drag.kind == 1
                    || (self.drag.kind == 2 && self.creative)
                {
                    self.drag.status = 1;
                    self.drag.slots.clear();
                } else {
                    self.drag = Drag::default();
                }
            }
            1 => {
                let Some(i) = slot else { return };
                let fits = self.drag.kind == 2 || carried.count() > self.drag.slots.len() as u32;
                if self.quick_replace(Self::at(i), &carried)
                    && fits
                    && Self::can_drag_to(i)
                    && !self.drag.slots.contains(&i)
                {
                    self.drag.slots.push(i);
                }
            }
            2 => {
                let slots = std::mem::take(&mut self.drag.slots);
                let kind = self.drag.kind;
                self.drag = Drag::default();
                if slots.len() == 1 {
                    self.pickup(Self::at(slots[0]), kind == 0);
                    return;
                }
                if slots.is_empty() {
                    return;
                }
                let mut left = carried.count();
                for &i in &slots {
                    let at = Self::at(i);
                    let fits = kind == 2 || carried.count() >= slots.len() as u32;
                    if !(self.quick_replace(at, &carried) && fits && Self::can_drag_to(i)) {
                        continue;
                    }
                    let have = self.get(at).map_or(0, Stack::count);
                    let each = match kind {
                        0 => carried.count() / slots.len() as u32,
                        1 => 1,
                        _ => carried.max(),
                    };
                    let n = (each + have).min(carried.max());
                    left = left.saturating_sub(n - have.min(n));
                    self.set(at, Some(carried.with_count(n)));
                }
                self.set_carried(Some(carried.with_count(left)));
            }
            _ => self.drag = Drag::default(),
        }
    }

    /// The `PICKUP_ALL` click (a double click): stacks like the carried one gather onto it,
    /// part stacks first (`canTakeItemForPickAll`: the player's slots but the held one).
    fn pickup_all(&mut self, index: usize, button: i32) {
        let Some(mut carried) = self.carried.clone() else {
            return;
        };
        if self.get(Self::at(index)).is_some() {
            return;
        }
        let order: Vec<usize> = if button == 0 {
            (0..self.slots()).collect()
        } else {
            (0..self.slots()).rev().collect()
        };
        for pass in 0..2 {
            for &i in &order {
                if carried.count() >= carried.max() {
                    break;
                }
                if i >= PLAYER_SLOTS || self.held_slot(i) {
                    continue;
                }
                let at = Self::at(i);
                let Some(s) = self.get(at).cloned() else {
                    continue;
                };
                if !self.quick_replace(at, &carried) || (pass == 0 && s.count() == s.max()) {
                    continue;
                }
                let n = s.count().min(carried.max() - carried.count());
                carried = carried.with_count(carried.count() + n);
                self.set(at, Some(s.with_count(s.count() - n)));
            }
        }
        self.carried = Some(carried);
    }

    /// `IClearableMenu.clearContents` of a list filter: its ghost slots emptied.
    pub fn clear(&mut self) {
        if self.kind == Kind::Filter {
            self.ghost.iter_mut().for_each(|g| *g = None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct S(&'static str, u32, u32);

    impl Stack for S {
        fn count(&self) -> u32 {
            self.1
        }
        fn with_count(&self, count: u32) -> Self {
            S(self.0, count, self.2)
        }
        fn max(&self) -> u32 {
            self.2
        }
        fn same(&self, other: &Self) -> bool {
            self.0 == other.0
        }
    }

    fn dirt(n: u32) -> S {
        S("dirt", n, 64)
    }

    fn filter() -> S {
        S("filter", 1, 1)
    }

    /// A list filter held in hotbar slot 0, with dirt in inventory slot 9 (menu slot 0).
    fn menu() -> Menu<S> {
        let mut inventory = vec![None; 36];
        inventory[0] = Some(filter());
        inventory[9] = Some(dirt(10));
        Menu::new(Kind::Filter, Vec::new(), inventory, 0)
    }

    #[test]
    fn a_ghost_slot_takes_a_copy_of_one_and_keeps_the_cursor() {
        let mut m = menu();
        m.click(0, 0, Click::Pickup);
        assert_eq!(m.carried, Some(dirt(10)));
        m.click(36 + 4, 0, Click::Pickup);
        assert_eq!(m.ghost[4], Some(dirt(1)));
        assert_eq!(m.carried, Some(dirt(10)));
        m.carried = None;
        m.click(36 + 4, 0, Click::Pickup);
        assert_eq!(m.ghost[4], None);
    }

    #[test]
    fn a_shift_click_copies_into_the_first_empty_slot_unless_it_is_there() {
        let mut m = menu();
        m.click(0, 0, Click::QuickMove);
        m.click(0, 0, Click::QuickMove);
        assert_eq!(m.ghost[0], Some(dirt(1)));
        assert_eq!(m.ghost[1], None);
        assert_eq!(m.inventory[9], Some(dirt(10)));
    }

    #[test]
    fn the_held_filter_takes_no_pickup_but_can_be_thrown() {
        let mut m = menu();
        m.click(27, 0, Click::Pickup);
        assert_eq!(m.carried, None);
        assert_eq!(m.inventory[0], Some(filter()));
        let thrown = m.click(27, 0, Click::Throw);
        assert_eq!(thrown, vec![filter()]);
        assert!(!m.still_valid());
    }

    #[test]
    fn swapping_the_filter_away_ends_the_menu_and_follows_the_stack() {
        let mut m = menu();
        m.click(0, 0, Click::Swap);
        assert_eq!(m.inventory[9], Some(filter()));
        assert_eq!(m.inventory[0], Some(dirt(10)));
        assert_eq!(m.holder, Some(9));
        assert!(!m.still_valid());
    }

    #[test]
    fn the_attribute_filters_tag_slot_takes_nothing() {
        let mut inventory = vec![None; 36];
        inventory[9] = Some(dirt(3));
        let mut m = Menu::new(
            Kind::AttributeFilter,
            vec![None, Some(S("tag", 1, 64))],
            inventory,
            0,
        );
        m.carried = Some(dirt(3));
        m.click(37, 0, Click::Pickup);
        assert_eq!(m.ghost[1], Some(S("tag", 1, 64)));
        m.carried = None;
        m.click(0, 0, Click::QuickMove);
        assert_eq!(m.ghost[0], Some(dirt(1)));
        m.click(1, 0, Click::QuickMove);
        assert_eq!(m.ghost[0], None);
    }

    #[test]
    fn a_right_click_takes_half_and_puts_one() {
        let mut m = menu();
        m.click(0, 1, Click::Pickup);
        assert_eq!(m.carried, Some(dirt(5)));
        assert_eq!(m.inventory[9], Some(dirt(5)));
        m.click(1, 1, Click::Pickup);
        assert_eq!(m.inventory[10], Some(dirt(1)));
        assert_eq!(m.carried, Some(dirt(4)));
    }

    #[test]
    fn a_drag_spreads_over_the_player_slots_and_copies_into_ghosts() {
        let mut m = menu();
        m.click(0, 0, Click::Pickup);
        m.click(OUTSIDE, 0, Click::QuickCraft);
        m.click(1, 1, Click::QuickCraft);
        m.click(36, 1, Click::QuickCraft);
        m.click(2, 1, Click::QuickCraft);
        m.click(OUTSIDE, 2, Click::QuickCraft);
        assert_eq!(m.inventory[10], Some(dirt(5)));
        assert_eq!(m.inventory[11], Some(dirt(5)));
        assert_eq!(m.ghost[0], Some(dirt(1)));
        assert_eq!(m.carried, None);
    }

    #[test]
    fn outside_drops_all_or_one() {
        let mut m = menu();
        m.click(0, 0, Click::Pickup);
        assert_eq!(m.click(OUTSIDE, 1, Click::Pickup), vec![dirt(1)]);
        assert_eq!(m.click(OUTSIDE, 0, Click::Pickup), vec![dirt(9)]);
        assert_eq!(m.carried, None);
    }

    #[test]
    fn a_double_click_gathers_but_never_the_held_filter() {
        let mut inventory = vec![None; 36];
        inventory[0] = Some(filter());
        inventory[9] = Some(dirt(3));
        inventory[10] = Some(dirt(64));
        inventory[11] = Some(dirt(4));
        let mut m = Menu::new(Kind::Filter, Vec::new(), inventory, 0);
        m.carried = Some(dirt(1));
        m.click(5, 0, Click::PickupAll);
        assert_eq!(m.carried, Some(dirt(64)));
        assert_eq!(m.inventory[9], None);
        assert_eq!(m.inventory[11], None);
        assert_eq!(m.inventory[10], Some(dirt(8)));
        assert_eq!(m.inventory[0], Some(filter()));
    }

    #[test]
    fn reset_empties_a_lists_slots() {
        let mut m = menu();
        m.click(0, 0, Click::QuickMove);
        m.clear();
        assert!(m.ghost.iter().all(Option::is_none));
    }
}
