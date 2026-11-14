//! Spouts and item drains, and the `create:filling` / `create:emptying` recipes they run.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::drain::Emptying;
use create_engine::spout::Filling;
use create_engine::tank::Fluid;
use mod_api::{HostTransaction, Identifier, ItemStack, Recipe, ServerHost};
use std::cell::RefCell;
use std::sync::Arc;

/// The filling and emptying recipes, and the game's tables they were read from: `/reload`
/// builds new ones.
struct Recipes {
    tables: [Arc<[Recipe]>; 2],
    emptying: Vec<(String, Emptying)>,
    filling: Vec<Filling>,
}

thread_local! {
    static RECIPES: RefCell<Option<Recipes>> = const { RefCell::new(None) };
}

/// Reads the recipes the first time a spout or drain needs them, and again after `/reload`.
fn ensure_recipes(host: &dyn ServerHost) {
    let tables = [
        host.recipes_of_type("create:emptying"),
        host.recipes_of_type("create:filling"),
    ];
    let same = RECIPES.with_borrow(|r| {
        r.as_ref()
            .is_some_and(|r| r.tables.iter().zip(&tables).all(|(a, b)| Arc::ptr_eq(a, b)))
    });
    if same {
        return;
    }
    let emptying = read_emptying(host, &tables[0]);
    let filling = read_filling(host, &tables[1]);
    RECIPES.with_borrow_mut(|r| {
        *r = Some(Recipes {
            tables,
            emptying,
            filling,
        })
    });
}

/// A recipe's fluid and amount: `{"amount", "fluid" | "tag" | "id"}`.
fn recipe_fluid(json: &mod_api::serde_json::Value) -> Option<(Fluid, u32)> {
    let amount = u32::try_from(json["amount"].as_u64()?).ok()?;
    let id = json["fluid"]
        .as_str()
        .or_else(|| json["tag"].as_str())
        .or_else(|| json["id"].as_str())?;
    Some((Fluid::of_id(id)?, amount))
}

/// The items an ingredient's JSON accepts.
fn ingredient_items(host: &dyn ServerHost, json: &mod_api::serde_json::Value) -> Vec<String> {
    let Ok(Some(i)) = host.recipe_ingredient(&json.to_string()) else {
        return Vec::new();
    };
    if i.order.is_empty() {
        i.items.into_iter().collect()
    } else {
        i.order
    }
}

fn result_item(host: &dyn ServerHost, json: &mod_api::serde_json::Value) -> Option<String> {
    let stack = host.recipe_result(&json.to_string()).ok()??;
    Some(stack.item().id(host))
}

/// `create:emptying`: an item, the fluid it gives and what is left.
fn read_emptying(host: &dyn ServerHost, recipes: &[Recipe]) -> Vec<(String, Emptying)> {
    let mut out = Vec::new();
    for r in recipes {
        let json = &*r.json;
        let Some(ingredient) = json["ingredients"].get(0) else {
            continue;
        };
        let items = ingredient_items(host, ingredient);
        let results = json["results"].as_array().cloned().unwrap_or_default();
        let Some((fluid, amount)) = results.iter().find_map(recipe_fluid) else {
            continue;
        };
        let leftover = results
            .iter()
            .find(|o| o.get("amount").is_none())
            .and_then(|o| result_item(host, o));
        for item in items {
            out.push((
                item,
                Emptying {
                    fluid,
                    amount,
                    leftover: leftover.clone(),
                },
            ));
        }
    }
    out
}

/// `create:filling`: an item and a fluid, what it becomes.
fn read_filling(host: &dyn ServerHost, recipes: &[Recipe]) -> Vec<Filling> {
    let mut out = Vec::new();
    for r in recipes {
        let json = &*r.json;
        let Some(ingredient) = json["ingredients"].get(0) else {
            continue;
        };
        let Some((fluid, amount)) = json["ingredients"].get(1).and_then(recipe_fluid) else {
            continue;
        };
        let Some(result) = json["results"].get(0).and_then(|o| result_item(host, o)) else {
            continue;
        };
        for item in ingredient_items(host, ingredient) {
            out.push(Filling {
                item,
                fluid,
                amount,
                result: result.clone(),
            });
        }
    }
    out
}

fn stack_of(host: &dyn ServerHost, item: &str, count: u32) -> Option<ItemStack> {
    let key = host.item_key(&Identifier::parse(item).ok()?)?;
    ItemStack::new(key, count)
}

/// Spouts holding fluid (`SpoutBlockEntity.whenItemHeld` on the depot two below, `tick`): the
/// held item is filled after the 20-tick cycle (at its 5th-last tick).
pub(super) fn tick_spouts(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let spouts: Vec<([i32; 3], create_engine::spout::Spout)> = with_world(|w| {
        w.dims[dim]
            .spouts
            .iter()
            .filter(|(_, s)| s.fluid.is_some() || s.processing >= 0)
            .map(|(p, s)| (*p, *s))
            .collect()
    })
    .unwrap_or_default();
    if spouts.is_empty() {
        return;
    }
    ensure_recipes(&*lv.host);
    for (pos, mut s) in spouts {
        let before = s.fluid;
        let depot = [pos[0], pos[1] - 2, pos[2]];
        let on_depot = with_world(|w| w.dims[dim].depots.contains_key(&depot)).unwrap_or(false);
        let entity = on_depot
            .then(|| lv.host.block_entity(lv.level, depot))
            .flatten();
        // The depot's held stack (slot 0).
        let held = entity
            .and_then(|e| e.item_slot(&*lv.host, 0).ok().flatten())
            .map(|stack| (stack.item().id(&*lv.host), stack.count()));
        let filled = RECIPES.with_borrow(|r| {
            let r = r.as_ref()?;
            let item = held.as_ref().map(|h| h.0.as_str());
            create_engine::spout::step(&mut s, item, &r.filling)
        });
        if let (Some(out), Some((item, count)), Some(entity)) = (filled, held, entity) {
            fill_held(lv.host, entity, &item, count, &out);
            with_world(|w| w.depot_changed(dim, depot));
        }
        with_world(|w| w.dims[dim].spouts.insert(pos, s));
        if s.fluid != before {
            with_world(|w| w.dims[dim].analog_checks.push(pos));
        }
    }
}

/// `convertToAndLeaveHeld`: one of the `count` held `item`s becomes `out`; the rest stays held,
/// the filled one goes to the outputs as the depot stacks them (`insertItemStacked`).
fn fill_held(
    host: &mut dyn ServerHost,
    entity: mod_api::BlockEntityRef,
    item: &str,
    count: u32,
    out: &str,
) {
    let Some(filled) = stack_of(&*host, out, 1) else {
        return;
    };
    let rest = if count > 1 {
        stack_of(&*host, item, count - 1)
    } else {
        None
    };
    let _ = host.transaction(|host, tx| {
        match rest {
            None => {
                entity
                    .set_item_slot(host, tx, 0, Some(filled))
                    .map_err(|_| ())?;
            }
            Some(rest) => {
                entity
                    .set_item_slot(host, tx, 0, Some(rest))
                    .map_err(|_| ())?;
                let outputs = create_engine::depot::OUTPUTS;
                let slots: Vec<(u32, Option<ItemStack>)> = outputs
                    .map(|i| (i as u32, entity.item_slot(&*host, i as u32).ok().flatten()))
                    .collect();
                let stacked = slots.iter().find(|(_, s)| {
                    s.as_ref()
                        .is_some_and(|s| s.is_stackable_with(&filled) && s.count() < 64)
                });
                if let Some((i, Some(s))) = stacked {
                    let mut s = s.clone();
                    s.grow(1).map_err(|_| ())?;
                    entity
                        .set_item_slot(host, tx, *i, Some(s))
                        .map_err(|_| ())?;
                } else if let Some((i, _)) = slots.iter().find(|(_, s)| s.is_none()) {
                    entity
                        .set_item_slot(host, tx, *i, Some(filled))
                        .map_err(|_| ())?;
                }
            }
        }
        Ok::<(), ()>(())
    });
}

/// Item drains holding items (`ItemDrainBlockEntity.tick`): a filled bucket is emptied into the
/// tank each cycle while it has room; the empty bucket is thrown off on top; anything else is
/// let go the same way.
pub(super) fn tick_drains(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let drains: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .drains
            .iter()
            .filter(|(_, d)| d.held.is_some())
            .map(|(p, _)| *p)
            .collect()
    })
    .unwrap_or_default();
    if drains.is_empty() {
        return;
    }
    ensure_recipes(&*lv.host);
    for pos in drains {
        let Some(out) = RECIPES.with_borrow(|r| {
            let emptying = r.as_ref().map_or(&[][..], |r| &r.emptying[..]);
            with_world(|w| {
                let d = w.dims[dim].drains.get_mut(&pos)?;
                let held = d.held.clone();
                if held
                    .as_ref()
                    .is_some_and(|h| create_engine::drain::contents(&h.0, emptying).is_none())
                {
                    d.held = None;
                    return held;
                }
                let before = d.fluid;
                let out = create_engine::drain::tick(d, emptying);
                if d.fluid != before {
                    w.dims[dim].analog_checks.push(pos);
                }
                out?.map(|left| (left, 1))
            })
            .flatten()
        }) else {
            continue;
        };
        let Some(stack) = stack_of(&*lv.host, &out.0, u32::from(out.1)) else {
            continue;
        };
        let (x, y, z) = (
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 1.0,
            f64::from(pos[2]) + 0.5,
        );
        // `summon minecraft:item ~ ~1 ~ {PickupDelay:20}`: no motion.
        let _ = lv
            .level
            .spawn_item_with_motion(&mut *lv.host, x, y, z, stack, [0.0; 3], 20);
    }
}
