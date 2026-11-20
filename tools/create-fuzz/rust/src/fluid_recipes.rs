//! The admitted component-free defaults, verified against Create's generated MIT recipes.
//! This is the same exact-item/fluid subset parsed by glue/processing/recipes.rs.
use create_engine::{drain::Emptying, spout::Filling, tank::Fluid};
pub fn emptying() -> Vec<(String, Emptying)> {
    vec![
        (
            "create:builders_tea".into(),
            Emptying {
                fluid: Fluid::Tea,
                amount: 250,
                leftover: Some("minecraft:glass_bottle".into()),
            },
        ),
        (
            "minecraft:honey_bottle".into(),
            Emptying {
                fluid: Fluid::Honey,
                amount: 250,
                leftover: Some("minecraft:glass_bottle".into()),
            },
        ),
    ]
}
pub fn filling() -> Vec<Filling> {
    vec![
        Filling {
            item: "minecraft:glass_bottle".into(),
            fluid: Fluid::Tea,
            amount: 250,
            result: "create:builders_tea".into(),
        },
        Filling {
            item: "minecraft:glass_bottle".into(),
            fluid: Fluid::Honey,
            amount: 250,
            result: "minecraft:honey_bottle".into(),
        },
        Filling {
            item: "minecraft:sweet_berries".into(),
            fluid: Fluid::Chocolate,
            amount: 250,
            result: "create:chocolate_glazed_berries".into(),
        },
    ]
}
pub fn contents(item: &str) -> Option<Emptying> {
    create_engine::drain::contents(item, &emptying())
}
