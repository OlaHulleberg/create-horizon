//! Postboxes' `open` (`PostboxBlockEntity.onOpenChange`, `AnimatedContainerBehaviour`): open
//! while at least one player has its menu open, with a barrel's sounds. What goes in a postbox
//! (packages, its address, trains calling at it) belongs to the package subsystem, not here.

use std::collections::HashMap;

/// Who has which postbox's menu open: by player and window, the postbox (its dimension index and
/// position), and how many menus each postbox has open (`openCount`).
#[derive(Default)]
pub struct Postboxes {
    windows: HashMap<(u128, i32), (usize, [i32; 3])>,
    open: HashMap<(usize, [i32; 3]), u32>,
}

impl Postboxes {
    /// Player `player` opened the postbox's menu as window `window` (`startOpen`): true when it
    /// is the first (`onOpenChange(true)`: the block opens, a barrel opens), and the postbox a
    /// window of the same id had open, when this was its last menu.
    pub fn opened(
        &mut self,
        player: u128,
        window: i32,
        dim: usize,
        pos: [i32; 3],
    ) -> (bool, Option<(usize, [i32; 3])>) {
        let left = self
            .windows
            .insert((player, window), (dim, pos))
            .filter(|was| self.close(*was));
        let n = self.open.entry((dim, pos)).or_default();
        *n += 1;
        (*n == 1, left)
    }

    /// The player's window closed (`stopOpen`): the postbox, when it was the last menu open on
    /// it (`onOpenChange(false)`).
    pub fn closed(&mut self, player: u128, window: i32) -> Option<(usize, [i32; 3])> {
        let at = self.windows.remove(&(player, window))?;
        self.close(at).then_some(at)
    }

    fn close(&mut self, at: (usize, [i32; 3])) -> bool {
        let Some(n) = self.open.get_mut(&at) else {
            return false;
        };
        *n -= 1;
        if *n == 0 {
            self.open.remove(&at);
            return true;
        }
        false
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_postbox_is_open_while_any_menu_on_it_is() {
        let mut p = Postboxes::default();
        let at = [3, 64, -2];
        assert_eq!(
            p.opened(1, 7, 0, at),
            (true, None),
            "the first menu opens it"
        );
        assert_eq!(
            p.opened(2, 4, 0, at),
            (false, None),
            "a second's doesn't again"
        );
        assert_eq!(p.closed(1, 7), None, "still open for the other");
        assert_eq!(p.closed(1, 7), None, "a window closes once");
        assert_eq!(p.closed(2, 4), Some((0, at)), "the last menu closes it");
        assert!(p.is_empty());
        // A player whose window is replaced by another postbox's leaves the first.
        assert_eq!(p.opened(1, 1, 0, at), (true, None));
        assert_eq!(p.opened(1, 1, 0, [0, 0, 0]), (true, Some((0, at))));
        assert_eq!(p.opened(1, 2, 0, at), (true, None), "the first was closed");
    }
}
