//! Brass funnels' filters and amounts, mirroring glue/logistics/{funnel,amount}.rs: a click on
//! the filter slot (`use_funnel_filter`) sets the filter from the held item while the amount
//! board does not show; with the board (`acceptsValueSettings`) the client sends the release
//! of a short click or a pick (`value_settings`). The filter lets only its item through
//! (`funnel_passes`); the board's count and mode say how much a funnel moves at once
//! (`funnel_extraction`). Filters are plain item ids.
use super::*;
use create_engine::{Block, board::FilterAmount, funnel::BeltShape, kinetics::Dir};

impl Run {
    /// The brass funnel at `p`: its facing and belt funnel shape (None for a plain funnel).
    fn brass_funnel(&self, p: Pos) -> Option<(Dir, Option<BeltShape>, bool)> {
        let (block, info) = self.w.table.get(self.at(p))?;
        let belt = match block {
            Block::BrassFunnel => None,
            Block::BrassBeltFunnel => Some(info.belt_funnel.unwrap_or_default()),
            _ => return None,
        };
        Some((info.facing?, belt, info.extracting))
    }
    /// `filter_max`: the filter's stack size, 64 without one.
    fn funnel_filter_max(&self, p: Pos) -> i32 {
        self.funnel_filters
            .get(&p)
            .map_or(64, |item| self.funnel_filter_max_of(item))
    }
    /// The stack size of a filter of `item` (64 for an empty hand).
    pub(super) fn funnel_filter_max_of(&self, item: &str) -> i32 {
        if item == "minecraft:air" {
            return 64;
        }
        i32::from(
            crate::transport::Stack {
                item: item.to_string(),
                count: 1,
            }
            .limit(),
        )
    }
    /// `funnel_supports` (`supportsAmountOnFilter`): a belt funnel pushing, or over a belt
    /// segment; an extracting funnel.
    fn funnel_supports_amount(&self, p: Pos, belt: Option<BeltShape>, extracting: bool) -> bool {
        match belt {
            Some(BeltShape::Pushing) => true,
            Some(_) => self.belt_location([p[0], p[1] - 1, p[2]]).is_some(),
            None => extracting,
        }
    }
    /// `amount::shown` (`isCountVisible`): the board's top value while it shows.
    fn funnel_amount_shown(&self, p: Pos) -> Option<i32> {
        let (_, belt, extracting) = self.brass_funnel(p)?;
        if !self.funnel_supports_amount(p, belt, extracting) {
            return None;
        }
        let max = self.funnel_filter_max(p);
        (max > 1).then_some(max)
    }
    /// `funnel_passes`: what the funnel's filter lets through (anything without one).
    pub(super) fn funnel_passes(&self, p: Pos, item: &str) -> bool {
        self.funnel_filters.get(&p).is_none_or(|f| f == item)
    }
    /// `funnel_extraction` (`getAmountToExtract`, `getModeToExtract`): a stack without an
    /// amount, one item for an andesite funnel, else the board's count, exactly on its second
    /// row.
    pub(super) fn funnel_extraction(&self, p: Pos) -> (u32, bool) {
        let Some((block, info)) = self.w.table.get(self.at(p)) else {
            return (64, false);
        };
        let belt = match block {
            Block::AndesiteBeltFunnel | Block::BrassBeltFunnel => {
                Some(info.belt_funnel.unwrap_or_default())
            }
            _ => None,
        };
        if !self.funnel_supports_amount(p, belt, info.extracting) {
            return (64, false);
        }
        if !matches!(block, Block::BrassFunnel | Block::BrassBeltFunnel) {
            return (1, false);
        }
        let a = self.filter_amounts.get(&p).copied().unwrap_or_default();
        let count = a.count(self.funnel_filter_max(p));
        (count.max(1) as u32, !a.up_to)
    }
    /// `set_funnel_filter` (`FilteringBehaviour.onShortInteract`): the held item becomes the
    /// filter, an empty hand clears it; an exact amount fits the new filter's stack size.
    pub(super) fn set_funnel_filter(&mut self, p: Pos, item: &str) {
        if item == "minecraft:air" {
            self.funnel_filters.remove(&p);
        } else {
            self.funnel_filters.insert(p, item.to_string());
        }
        let max = self.funnel_filter_max(p);
        if let Some(a) = self.filter_amounts.get_mut(&p) {
            *a = a.fitted(max);
        }
        self.wake_funnel(p);
        self.w.mark_unsaved(0, p);
    }
    /// `wake_funnel` (`invVersionTracker.reset`): the funnel looks at its inventory again.
    fn wake_funnel(&mut self, p: Pos) {
        let d = &mut self.w.dims[0];
        if let Some(due) = d.funnels.get_mut(&p) {
            *due = 0;
        }
    }
    /// `use_funnel_filter`: a click on a brass funnel's filter slot, not sneaking, with neither
    /// a wrench nor an arm; it sets the filter unless the amount board shows (the client then
    /// waits for the release). True when the click was the slot's.
    pub(super) fn funnel_filter_use(
        &mut self,
        p: Pos,
        item: &str,
        face: Dir,
        hit: [f64; 3],
        sneaking: bool,
    ) -> bool {
        let Some((facing, belt, _)) = self.brass_funnel(p) else {
            return false;
        };
        if sneaking || matches!(item, "create:wrench" | "create:mechanical_arm") {
            return false;
        }
        let Some(slot) = create_engine::funnel::filter_slot(facing, belt, face) else {
            return false;
        };
        if !create_engine::board::hits_box(slot, hit) {
            return false;
        }
        if self.funnel_amount_shown(p).is_none() {
            self.set_funnel_filter(p, item);
        }
        true
    }
    /// `amount::value_settings` for a brass funnel (only while its board shows): a short
    /// click's release sets the filter from `interact`'s item (a wrench or an arm does
    /// nothing), a pick sets the amount.
    pub(super) fn funnel_value_settings(
        &mut self,
        p: Pos,
        row: i32,
        value: i32,
        interact: Option<&str>,
    ) {
        if self.brass_funnel(p).is_none() {
            return;
        }
        let Some(max) = self.funnel_amount_shown(p) else {
            return;
        };
        match interact {
            Some("create:wrench" | "create:mechanical_arm") => {}
            Some(item) => self.set_funnel_filter(p, item),
            None => {
                let now = self.filter_amounts.get(&p).copied().unwrap_or_default();
                let next =
                    FilterAmount::from_settings(create_engine::board::Settings { row, value }, max);
                if now.settings(max) != next.settings(max) {
                    self.filter_amounts.insert(p, next);
                    self.wake_funnel(p);
                    self.w.mark_unsaved(0, p);
                }
            }
        }
    }
    /// The funnel observation's filter and amount (`FilteringBehaviour.count` within the
    /// filter's stack size, `upTo`) for a brass funnel.
    pub(super) fn funnel_filter_observation(&self, p: Pos) -> Option<(Option<String>, i32, bool)> {
        self.brass_funnel(p)?;
        let a = self.filter_amounts.get(&p).copied().unwrap_or_default();
        let max = self.funnel_filter_max(p);
        Some((
            self.funnel_filters.get(&p).cloned(),
            a.count(max).min(max),
            a.up_to,
        ))
    }
}
