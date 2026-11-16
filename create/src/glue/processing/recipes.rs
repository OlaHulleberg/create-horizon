//! Create's recipes of the types its machines run, read once per data load (`tags_updated`,
//! after the server published its recipes and tags, on first open and after each `/reload`):
//! typed and indexed by input item, as the engine's books. Matching runs on these; a crafting
//! grid is matched by the game (`LevelRef::recipe_for`).

use crate::glue::server::ServerLevel;
use create_engine::processing::{Book, Output, Recipe};
use create_engine::{basin, deployer, drain, fan, runtime_data, sequenced, spout, tank};
use mod_api::serde_json::Value;
use mod_api::{Host, Identifier, ItemStack, RecipeInput, RecipeRule};
use std::cell::RefCell;

/// Create's recipe books, by the machine that runs them.
pub struct Recipes {
    pub milling: Book,
    pub pressing: Book,
    pub crushing: Book,
    /// Create's cutting recipes (the saw), then vanilla's stonecutting ones
    /// (`allowStonecuttingOnSaw`, on by default).
    pub cutting: Book,
    /// Create's mixing recipes, then shapeless crafting recipes of more than one ingredient
    /// the press doesn't compress (`allowShapelessInMixer`).
    pub mixing: Vec<basin::Recipe>,
    /// Create's compacting recipes (a press over a basin).
    pub compacting: Vec<basin::Recipe>,
    /// The fan's: Create's washing and haunting, vanilla's cooking.
    pub splashing: Book,
    pub haunting: Book,
    pub smoking: Book,
    pub smelting: Book,
    pub blasting: Book,
    /// The deployer's: `create:deploying`, then `create:item_application` (automatable ones).
    pub deploying: Vec<deployer::Recipe>,
    /// `create:item_application`, the manual-only ones too: a player's item on a block.
    pub manual_application: Vec<deployer::Recipe>,
    /// The item drain's and the spout's: `create:emptying`, `create:filling` with the fluids
    /// the machines carry (the fluids subsystem's machines read them).
    pub emptying: Vec<(String, drain::Emptying)>,
    pub filling: Vec<spout::Filling>,
    /// `create:sequenced_assembly`: steps the machines ask for first.
    pub sequenced: Vec<sequenced::Assembly>,
    /// `create:mechanical_crafting`, shaped.
    pub mechanical_crafting: Vec<create_engine::crafter::Shaped>,
    /// `create:sandpaper_polishing` (sand paper in a player's hands).
    pub polishing: Book,
}

impl Recipes {
    pub fn fan(&self) -> fan::Books<'_> {
        fan::Books {
            splashing: &self.splashing,
            haunting: &self.haunting,
            smoking: &self.smoking,
            smelting: &self.smelting,
            blasting: &self.blasting,
        }
    }
}

/// Create's recipe serializers (`AllRecipeTypes`), each declaring a recipe type of the same id:
/// the game loads a recipe file only through a registered serializer, and lists it under its
/// serializer's type (`recipes_of_type`), the file as written. `toolbox_dyeing` and
/// `item_copying` are crafting recipes in Create, special ones no grid here runs: they keep a
/// type of their own, as their files were listed before.
const SERIALIZERS: [&str; 19] = [
    "create:conversion",
    "create:crushing",
    "create:cutting",
    "create:milling",
    "create:basin",
    "create:mixing",
    "create:compacting",
    "create:pressing",
    "create:sandpaper_polishing",
    "create:splashing",
    "create:haunting",
    "create:deploying",
    "create:filling",
    "create:emptying",
    "create:item_application",
    "create:mechanical_crafting",
    "create:sequenced_assembly",
    "create:toolbox_dyeing",
    "create:item_copying",
];

/// Declares Create's recipe types and their serializers, which read a file as its JSON.
pub fn register(r: &mut dyn mod_api::Registrar) {
    for id in SERIALIZERS {
        r.register_recipe_type(id);
        r.register_raw_recipe_serializer(id, id);
    }
}

/// The crafting recipes whose file is of serializer `serializer` (`minecraft:crafting_shaped`,
/// `minecraft:crafting_shapeless`): the game lists every crafting serializer's recipes under
/// `minecraft:crafting`.
pub fn crafting_of(host: &dyn Host, serializer: &str) -> Vec<mod_api::Recipe> {
    host.recipes_of_type("minecraft:crafting")
        .iter()
        .filter(|r| r.json["type"].as_str() == Some(serializer))
        .cloned()
        .collect()
}

thread_local! {
    static RECIPES: RefCell<Option<Recipes>> = const { RefCell::new(None) };
}

/// Runs `f` on the recipes in force; None before they are read or without Create's data.
pub fn with<R>(f: impl FnOnce(&Recipes) -> R) -> Option<R> {
    RECIPES.with_borrow(|r| r.as_ref().map(f))
}

/// Whether there are recipes to run (Create's data is installed and has been read).
pub fn ready() -> bool {
    RECIPES.with_borrow(Option::is_some)
}

/// Forgets the recipes (the world closed).
pub fn clear() {
    RECIPES.with_borrow_mut(|r| *r = None);
}

/// Reads every book from the server's recipes in force and replaces the old ones at once.
/// None at all when Create's milling and pressing recipes are missing (its data isn't there).
pub fn publish(host: &dyn Host) {
    let milling = read_book(host, "create:milling");
    let pressing = read_book(host, "create:pressing");
    if milling.is_empty() && pressing.is_empty() {
        clear();
        return;
    }
    // `RuntimeDataGenerator`'s recipes (wood cutting, concrete washing), made from the items.
    let items: Vec<String> = host.item_keys().into_iter().map(|k| k.id(host)).collect();
    let mut cutting = read_book(host, "create:cutting").recipes;
    cutting.extend(runtime_data::cutting(&items));
    cutting.extend(read_stonecutting(host));
    let mut splashing = read_book(host, "create:splashing").recipes;
    splashing.extend(runtime_data::splashing(&items));
    let mut mixing = read_basin(host, "create:mixing");
    mixing.extend(read_shapeless_mixing(host));
    let potions = read_potion_mixing(host);
    mixing.extend(potions);
    let next = Recipes {
        milling,
        pressing,
        crushing: read_book(host, "create:crushing"),
        cutting: Book::new(cutting),
        mixing,
        compacting: read_basin(host, "create:compacting"),
        splashing: Book::new(splashing),
        haunting: read_book(host, "create:haunting"),
        smoking: read_cooking(host, "minecraft:smoking"),
        smelting: read_cooking(host, "minecraft:smelting"),
        blasting: read_cooking(host, "minecraft:blasting"),
        deploying: ["create:deploying", "create:item_application"]
            .into_iter()
            .flat_map(|k| read_application(host, k, false))
            .collect(),
        manual_application: read_application(host, "create:item_application", true),
        emptying: read_emptying(host),
        filling: read_filling(host),
        sequenced: read_sequenced(host),
        mechanical_crafting: read_mechanical_crafting(host),
        polishing: read_book(host, "create:sandpaper_polishing"),
    };
    RECIPES.with_borrow_mut(|r| *r = Some(next));
}

/// An ingredient's items in its order, from recipe-file JSON: an item, a `#tag` or a list of
/// them (the game's reading), or a `neoforge:block_tag` as Create's data writes it (the items
/// of the tag's blocks, in the host's block order), which the game reads only in its own recipe
/// types.
fn ingredient(host: &dyn Host, json: &Value) -> Option<Vec<String>> {
    if json["type"].as_str() == Some("neoforge:block_tag") {
        let tag = json["tag"].as_str()?;
        let items: Vec<String> = host
            .block_keys()
            .into_iter()
            .filter(|&b| host.block_in_tag(b, tag) == Some(true))
            .filter_map(|b| host.block_item(b))
            .map(|i| i.id(host))
            .filter(|i| i != "minecraft:air")
            .collect();
        return (!items.is_empty()).then_some(items);
    }
    let ing = host.recipe_ingredient(&json.to_string()).ok()??;
    Some(ing.order)
}

/// A result stack from recipe-file JSON.
fn result(host: &dyn Host, json: &Value) -> Option<ItemStack> {
    host.recipe_result(&json.to_string()).ok()?
}

fn output(host: &dyn Host, o: &Value) -> Option<Output> {
    let stack = result(host, o)?;
    Some(Output {
        item: super::stacks::id(host, &stack),
        count: stack.count().min(255) as u8,
        chance: o["chance"].as_f64().unwrap_or(1.0) as f32,
    })
}

/// A recipe's fluid (`FluidIngredient` or a result): its fluid by id or tag, and amount.
pub fn recipe_fluid(json: &Value) -> Option<(tank::Fluid, u32)> {
    let amount = u32::try_from(json["amount"].as_u64()?).ok()?;
    let id = json["fluid"]
        .as_str()
        .or_else(|| json["tag"].as_str())
        .or_else(|| json["id"].as_str())
        .or_else(|| json["fluids"].as_str())?;
    // A potion fluid named by its components (`neoforge:components`: the potion and its
    // bottle, `DataComponentFluidIngredient`), as the spout's glowstone, redstone and
    // gunpowder recipes take.
    if id == "create:potion" {
        let c = &json["components"];
        let potion = c["minecraft:potion_contents"]["potion"].as_str()?;
        let bottle = match c["create:potion_fluid_bottle_type"]
            .as_str()
            .unwrap_or("regular")
        {
            "splash" => tank::Bottle::Splash,
            "lingering" => tank::Bottle::Lingering,
            _ => tank::Bottle::Regular,
        };
        return Some((
            tank::Fluid::Potion(tank::Potion::of(potion, bottle)?),
            amount,
        ));
    }
    Some((tank::Fluid::of_id(id)?, amount))
}

fn read_book(host: &dyn Host, kind: &str) -> Book {
    let recipes = host
        .recipes_of_type(kind)
        .iter()
        .filter_map(|r| {
            let json = &*r.json;
            let ingredients = json["ingredients"]
                .as_array()?
                .iter()
                .map(|i| ingredient(host, i))
                .collect::<Option<Vec<_>>>()?;
            let results = json["results"]
                .as_array()?
                .iter()
                .map(|o| output(host, o))
                .collect::<Option<Vec<_>>>()?;
            Some(Recipe {
                id: r.id.clone(),
                ingredients,
                results,
                time: json["processing_time"].as_i64().unwrap_or(0) as i32,
            })
        })
        .collect();
    Book::new(recipes)
}

/// A vanilla cooking recipe type (`minecraft:smelting`) as a book: its one ingredient, its one
/// result, as the game parsed them.
fn read_cooking(host: &dyn Host, kind: &str) -> Book {
    let recipes = host
        .recipes_of_type(kind)
        .iter()
        .filter_map(|r| {
            let input = r.ingredients.first()?;
            let out = r.results.first()?;
            Some(Recipe {
                id: r.id.clone(),
                ingredients: vec![input.order.clone()],
                results: vec![Output {
                    item: super::stacks::id(host, out),
                    count: out.count().min(255) as u8,
                    chance: 1.0,
                }],
                time: 0,
            })
        })
        .collect();
    Book::new(recipes)
}

/// `SawBlockEntity.getRecipes` with `allowStonecuttingOnSaw`: vanilla's stonecutting recipes
/// cut on a saw too, after Create's cutting ones, each 50 ticks (`start`: only a cutting
/// recipe has its own time).
fn read_stonecutting(host: &dyn Host) -> Vec<Recipe> {
    host.recipes_of_type("minecraft:stonecutting")
        .iter()
        .filter_map(|r| {
            let Some(RecipeRule::Stonecutting { input, result }) = &r.rule else {
                return None;
            };
            Some(Recipe {
                id: r.id.clone(),
                ingredients: vec![input.order.clone()],
                results: vec![Output {
                    item: super::stacks::id(host, result),
                    count: result.count().min(64) as u8,
                    chance: 1.0,
                }],
                time: 50,
            })
        })
        .collect()
}

/// `MechanicalMixerBlockEntity.matchStaticFilters`: shapeless crafting recipes of more than one
/// ingredient that the press doesn't compress (`canCompress`: 4 or 9 of the same) mix in a
/// basin too, after Create's own.
fn read_shapeless_mixing(host: &dyn Host) -> Vec<basin::Recipe> {
    let mut found = Vec::new();
    for r in crafting_of(host, "minecraft:crafting_shapeless").iter() {
        if r.ingredients.len() < 2 || r.id.ends_with("_manual_only") {
            continue;
        }
        let ingredients: Vec<Vec<String>> = r.ingredients.iter().map(|i| i.order.clone()).collect();
        let same = ingredients.windows(2).all(|w| w[0] == w[1]);
        if same && matches!(ingredients.len(), 4 | 9) {
            continue;
        }
        let Some(out) = r.results.first() else {
            continue;
        };
        found.push(basin::Recipe {
            id: r.id.clone(),
            ingredients,
            results: vec![Output {
                item: super::stacks::id(host, out),
                count: out.count().min(64) as u8,
                chance: 1.0,
            }],
            time: 0,
            heat: Default::default(),
            fluids: Vec::new(),
            fluid_results: Vec::new(),
        });
    }
    found
}

/// `PotionMixingRecipes` (`allowBrewingInMixer`, on by default): each brewing recipe from one
/// potion bottle to another (`minecraft:brewing`, which the game reads from data, its potion,
/// splash and lingering ones listed apiece) as a heated mixing recipe, its reagent with
/// 1000 mB of the potion fluid it starts from making 1000 mB of the one it ends at (a regular
/// water bottle's is water).
fn read_potion_mixing(host: &dyn Host) -> Vec<basin::Recipe> {
    use create_engine::tank::{potion_fluid, potion_item};
    let mut found = Vec::new();
    for r in host.recipes_of_type("minecraft:brewing").iter() {
        let json = &*r.json;
        let (Some(from_item), Some(to_item)) = (
            json["input"]["item"].as_str(),
            json["output"]["id"].as_str(),
        ) else {
            continue;
        };
        let Some(to_potion) =
            json["output"]["components"]["minecraft:potion_contents"]["potion"].as_str()
        else {
            continue;
        };
        let Some(to) = potion_fluid(&potion_item(to_item, to_potion)) else {
            continue;
        };
        // A brewing reagent names its items as `item` (an id, a `#tag` or a list).
        let Some(reagent) = ingredient(host, &json["reagent"]["item"]) else {
            continue;
        };
        let potions: Vec<&str> = match &json["input"]["potion_contents"]["potions"] {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for from_potion in potions {
            let Some(from) = potion_fluid(&potion_item(from_item, from_potion)) else {
                continue;
            };
            found.push(basin::Recipe {
                id: r.id.clone(),
                ingredients: vec![reagent.clone()],
                results: Vec::new(),
                time: 0,
                heat: basin::HeatReq::Heated,
                fluids: vec![(from, 1000)],
                fluid_results: vec![(to, 1000)],
            });
        }
    }
    found
}

/// A basin recipe type: item and fluid ingredients and results, with its `heat_requirement`.
fn read_basin(host: &dyn Host, kind: &str) -> Vec<basin::Recipe> {
    host.recipes_of_type(kind)
        .iter()
        .filter_map(|r| {
            let json = &*r.json;
            let mut ingredients = Vec::new();
            let mut fluids = Vec::new();
            for i in json["ingredients"].as_array()? {
                if i.get("amount").is_some() {
                    fluids.push(recipe_fluid(i)?);
                } else {
                    ingredients.push(ingredient(host, i)?);
                }
            }
            let mut results = Vec::new();
            let mut fluid_results = Vec::new();
            for o in json["results"].as_array()? {
                if o.get("amount").is_some() {
                    fluid_results.push(recipe_fluid(o)?);
                    continue;
                }
                results.push(output(host, o)?);
            }
            Some(basin::Recipe {
                id: r.id.clone(),
                ingredients,
                results,
                time: json["processing_time"].as_i64().unwrap_or(0) as i32,
                heat: basin::HeatReq::read(json["heat_requirement"].as_str()),
                fluids,
                fluid_results,
            })
        })
        .collect()
}

/// `ItemApplicationRecipe`s: the item worked on, the one held, the results; `_manual_only`
/// ones left out (`CAN_BE_AUTOMATED`).
fn read_application(host: &dyn Host, kind: &str, manual: bool) -> Vec<deployer::Recipe> {
    host.recipes_of_type(kind)
        .iter()
        .filter(|r| manual || !r.id.ends_with("_manual_only"))
        .filter_map(|r| {
            let json = &*r.json;
            let mut ingredients = json["ingredients"]
                .as_array()?
                .iter()
                .map(|i| ingredient(host, i));
            let target = ingredients.next()??;
            let tool = ingredients.next()??;
            let results = json["results"]
                .as_array()?
                .iter()
                .map(|o| output(host, o))
                .collect::<Option<Vec<_>>>()?;
            Some(deployer::Recipe {
                id: r.id.clone(),
                target,
                tool,
                results,
                keep_held: json["keep_held_item"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// `create:emptying`: each item its fluid and what is left of it.
fn read_emptying(host: &dyn Host) -> Vec<(String, drain::Emptying)> {
    let mut out = Vec::new();
    for r in host.recipes_of_type("create:emptying").iter() {
        let json = &*r.json;
        let Some(items) = json["ingredients"].get(0).and_then(|i| ingredient(host, i)) else {
            continue;
        };
        let results = json["results"].as_array().cloned().unwrap_or_default();
        let Some((fluid, amount)) = results.iter().find_map(recipe_fluid) else {
            continue;
        };
        let leftover = results
            .iter()
            .find(|o| o.get("amount").is_none())
            .and_then(|o| result(host, o))
            .map(|s| super::stacks::id(host, &s));
        for item in items {
            out.push((
                item,
                drain::Emptying {
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
fn read_filling(host: &dyn Host) -> Vec<spout::Filling> {
    let mut out = Vec::new();
    for r in host.recipes_of_type("create:filling").iter() {
        let json = &*r.json;
        let Some(items) = json["ingredients"].get(0).and_then(|i| ingredient(host, i)) else {
            continue;
        };
        let Some((fluid, amount)) = json["ingredients"].get(1).and_then(recipe_fluid) else {
            continue;
        };
        let Some(made) = json["results"].get(0).and_then(|o| result(host, o)) else {
            continue;
        };
        let made = super::stacks::id(host, &made);
        for item in items {
            out.push(spout::Filling {
                item,
                fluid,
                amount,
                result: made.clone(),
            });
        }
    }
    out
}

/// `create:sequenced_assembly` (`SequencedAssemblyRecipeSerializer`): the starting items, the
/// transitional item, each step's recipe type with a deployer's second ingredient or a spout's
/// fluid, the loops (1 unset) and the weighted results.
fn read_sequenced(host: &dyn Host) -> Vec<sequenced::Assembly> {
    use sequenced::{Assembly, Outcome, Step};
    host.recipes_of_type("create:sequenced_assembly")
        .iter()
        .filter_map(|r| {
            let json = &*r.json;
            let starting = ingredient(host, &json["ingredient"])?;
            let transitional = json["transitional_item"]["id"]
                .as_str()
                .or_else(|| json["transitional_item"]["item"].as_str())?
                .to_string();
            let steps = json["sequence"]
                .as_array()?
                .iter()
                .map(|step| {
                    let ingredients = step["ingredients"].as_array();
                    let second = ingredients.and_then(|i| i.get(1));
                    let fluid = second
                        .filter(|i| i.get("amount").is_some())
                        .and_then(recipe_fluid);
                    let with = second
                        .filter(|i| i.get("amount").is_none())
                        .and_then(|i| ingredient(host, i));
                    Some(Step {
                        kind: step["type"].as_str()?.to_string(),
                        with,
                        fluid: fluid.map(|(f, n)| (f.id().to_string(), n)),
                        keep_held: step["keep_held_item"].as_bool().unwrap_or(false),
                        time: step["processing_time"].as_i64().unwrap_or(0) as i32,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let results = json["results"]
                .as_array()?
                .iter()
                .map(|o| {
                    let stack = result(host, o)?;
                    Some(Outcome {
                        item: super::stacks::id(host, &stack),
                        count: stack.count().min(255) as u8,
                        weight: o["chance"].as_f64().unwrap_or(1.0) as f32,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Assembly {
                id: r.id.clone(),
                ingredient: starting,
                transitional,
                steps,
                loops: json["loops"].as_u64().unwrap_or(1) as u32,
                results,
            })
        })
        .collect()
}

/// `create:mechanical_crafting`: a pattern of up to any size, its key, `accept_mirrored`.
fn read_mechanical_crafting(host: &dyn Host) -> Vec<create_engine::crafter::Shaped> {
    host.recipes_of_type("create:mechanical_crafting")
        .iter()
        .filter_map(|r| {
            let json = &*r.json;
            let pattern: Vec<&str> = json["pattern"]
                .as_array()?
                .iter()
                .filter_map(|l| l.as_str())
                .collect();
            let height = pattern.len();
            let width = pattern.iter().map(|l| l.chars().count()).max()?;
            let mut cells = Vec::with_capacity(width * height);
            for line in &pattern {
                let chars: Vec<char> = line.chars().collect();
                for c in 0..width {
                    let ch = chars.get(c).copied().unwrap_or(' ');
                    cells.push(if ch == ' ' {
                        None
                    } else {
                        Some(ingredient(host, &json["key"][ch.to_string()])?)
                    });
                }
            }
            let out = result(host, &json["result"])?;
            Some(create_engine::crafter::Shaped {
                width,
                height,
                cells,
                mirrored: json["accept_mirrored"].as_bool().unwrap_or(true),
                result: (super::stacks::id(host, &out), out.count().min(64) as u8),
            })
        })
        .collect()
}

/// `RecipeManager.getRecipeFor(CRAFTING, ..)` on a square grid of `size` (row-major, one item
/// each): the result, or None.
pub fn craft(lv: &ServerLevel<'_>, grid: &[Option<String>], size: usize) -> Option<(String, u8)> {
    let host = &*lv.host;
    let slots = grid
        .iter()
        .map(|c| c.as_ref().and_then(|i| super::stacks::stack(host, i, 1)))
        .collect();
    let input = RecipeInput::Crafting {
        width: size as u32,
        height: size as u32,
        slots,
    };
    let kind = Identifier::parse("minecraft:crafting").ok()?;
    let made = lv.level.recipe_for(host, &kind, &input).ok()??;
    Some((
        super::stacks::id(host, &made.output),
        made.output.count().min(64) as u8,
    ))
}
