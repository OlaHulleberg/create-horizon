//! Package subsystem's explicit conveyor connections and kinetic synchronization.
use super::*;
impl Run {
    pub(super) fn sync_conveyors(&mut self) {
        for (&p, c) in &self.chains.conveyors {
            self.w.dims[0]
                .kinetics
                .set_far(p, c.connections.iter().map(|q| add(p, *q)).collect());
        }
    }
    pub(super) fn conveyor_changed(&mut self, p: Pos, old: u32, new: u32) {
        let conveyor = |s| {
            self.w
                .table
                .get(s)
                .is_some_and(|(b, _)| b == create_engine::Block::ChainConveyor)
        };
        if conveyor(old) && !conveyor(new) {
            self.chains.remove(p, &mut vec![]);
            self.mark_column(p); // packages::GONE marks the removed conveyor column.
            self.w.dims[0].kinetics.set_far(p, vec![]);
            self.sync_conveyors();
        } else if conveyor(new) && !conveyor(old) {
            self.chains.add(p, vec![]);
        }
    }
    pub(super) fn chain_link(&mut self, a: Pos, b: Pos, on: bool) {
        if on {
            self.chains.connect(a, b);
        } else {
            self.chains.disconnect(a, b, &mut vec![]);
        }
        self.sync_conveyors();
    }
}
