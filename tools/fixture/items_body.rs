// Stand-in for `mixins/create/items_body.rs` in the template test: the fixture's stacks carry no
// components, so the settings the debug commands write are kept here by item id (one stack of
// each item at a time is enough for the test), and items have no facts beyond their id.
use super::Game;
use create_engine::filter::{Attribute, Mode, Spec};
use create_engine::symmetry::Wand;
use create_engine::worldshaper as ws;
use horizon_client::items::ItemStack;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, Default)]
struct Data {
    list: Option<(Vec<String>, bool, bool)>,
    attributes: Option<(Mode, Vec<(Attribute, bool)>)>,
    wand: Option<Wand>,
    shaper: Option<(ws::Settings, Option<String>)>,
    schedule: Option<Vec<(String, i32)>>,
    belt_first: Option<[i32; 3]>,
    controller: Vec<String>,
    contraption: Option<horizon_world::nbt::Compound>,
}

thread_local! {
    static DATA: RefCell<HashMap<String, Data>> = RefCell::new(HashMap::new());
}

fn data(stack: &ItemStack) -> Data {
    DATA.with_borrow(|d| d.get(&*stack.item).cloned().unwrap_or_default())
}

fn edit(stack: &ItemStack, f: impl FnOnce(&mut Data)) {
    DATA.with_borrow_mut(|d| f(d.entry(stack.item.to_string()).or_default()));
}

pub(super) fn components(stack: &ItemStack) -> String {
    let d = data(stack);
    if d.list.is_some() || d.attributes.is_some() {
        "{create}".into()
    } else {
        String::new()
    }
}

pub(super) fn spec(stack: &ItemStack) -> Spec {
    let d = data(stack);
    let (list, respect_nbt, blacklist) = d.list.clone().unwrap_or_default();
    Spec {
        item: stack.item.to_string(),
        components: components(stack),
        has_list: d.list.is_some(),
        list: list.iter().map(|i| Spec::item(i)).collect(),
        respect_nbt,
        blacklist,
        mode: d.attributes.as_ref().map(|a| a.0.name().to_owned()),
        attributes: d
            .attributes
            .map(|(_, t)| t.iter().map(|(a, i)| (a.id(), a.value(), *i)).collect()),
        address: String::new(),
    }
}

pub(super) fn set_list(stack: &mut ItemStack, items: &[ItemStack], respect_nbt: bool, deny: bool) {
    let items = items.iter().map(|s| s.item.to_string()).collect();
    edit(stack, |d| d.list = Some((items, respect_nbt, deny)));
}

pub(super) fn set_attributes(stack: &mut ItemStack, mode: Mode, tests: &[(Attribute, bool)]) {
    edit(stack, |d| d.attributes = Some((mode, tests.to_vec())));
}

/// Only an item's id: `added_by` and `placeable` (the fixture's stone).
pub(super) fn attribute(game: &Game<'_>, stack: &ItemStack, a: &Attribute) -> bool {
    let _ = game;
    match a {
        Attribute::AddedBy(m) => stack.item.split_once(':').map_or("minecraft", |s| s.0) == m,
        Attribute::Placeable => &*stack.item == "minecraft:stone",
        _ => false,
    }
}

pub(super) fn wand(stack: &ItemStack) -> Wand {
    data(stack).wand.unwrap_or_else(Wand::blank)
}

pub(super) fn set_wand(stack: &mut ItemStack, w: &Wand) {
    edit(stack, |d| d.wand = Some(*w));
}

pub(super) fn shaper(stack: &ItemStack) -> (ws::Settings, Option<String>) {
    data(stack).shaper.unwrap_or_default()
}

pub(super) fn set_shaper(stack: &mut ItemStack, s: &ws::Settings, block: Option<&str>) {
    let block = block.map(str::to_owned);
    edit(stack, |d| {
        let old = d.shaper.take().and_then(|x| x.1);
        d.shaper = Some((s.clone(), block.or(old)));
    });
}

pub(super) fn schedule(stack: &ItemStack) -> Option<Vec<(String, i32)>> {
    data(stack).schedule
}

pub(super) fn set_schedule(stack: &mut ItemStack, entries: &[(String, i32)]) {
    edit(stack, |d| d.schedule = Some(entries.to_vec()));
}

pub(super) fn belt_first_shaft(stack: &ItemStack) -> Option<[i32; 3]> {
    data(stack).belt_first
}

pub(super) fn set_belt_first_shaft(stack: &mut ItemStack, pos: Option<[i32; 3]>) {
    edit(stack, |d| d.belt_first = pos);
}

pub(super) fn controller_items(stack: &ItemStack) -> Vec<String> {
    data(stack).controller
}

pub(super) fn set_controller_items(stack: &mut ItemStack, items: &[String]) {
    edit(stack, |d| d.controller = items.to_vec());
}

pub(super) fn minecart_contraption(stack: &ItemStack) -> Option<horizon_world::nbt::Compound> {
    data(stack).contraption
}

pub(super) fn set_minecart_contraption(stack: &mut ItemStack, data: horizon_world::nbt::Compound) {
    edit(stack, |d| d.contraption = Some(data));
}

/// Into the first empty inventory slot.
pub(super) fn give_back(game: &mut Game<'_>, stack: ItemStack) {
    match game.inventory.iter_mut().find(|s| s.is_none()) {
        Some(slot) => *slot = Some(stack),
        None => game.inventory.push(Some(stack)),
    }
}

pub(super) fn sequenced_progress(stack: &ItemStack) -> Option<(String, u32)> {
    let _ = stack;
    None
}

pub(super) fn set_sequenced_progress(stack: &mut ItemStack, id: &str, step: u32, progress: f32) {
    let _ = (stack, id, step, progress);
}
