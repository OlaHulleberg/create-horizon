//! Create's equipment items (`AllItems`, `AllArmorMaterials`, `content/equipment`): the armour's
//! numbers as item components over vanilla templates. Their behaviour is the native mod's.

use serde_json::{Value, json};

/// `AllArmorMaterials.COPPER`'s equip sound (`COPPER_ARMOR_EQUIP` plays `ARMOR_EQUIP_GOLD`).
const COPPER_EQUIP: &str = "minecraft:item.armor.equip_gold";

/// The armour's attribute modifiers as `ArmorMaterial.createAttributes` gives them.
fn armor(slot: &str, kind: &str, defense: f64, toughness: f64, knockback: f64) -> Value {
    let id = format!("minecraft:armor.{kind}");
    let mut v = vec![
        json!({ "type": "minecraft:armor", "amount": defense, "id": id,
                "operation": "add_value", "slot": slot }),
        json!({ "type": "minecraft:armor_toughness", "amount": toughness, "id": id,
                "operation": "add_value", "slot": slot }),
    ];
    if knockback > 0.0 {
        v.push(
            json!({ "type": "minecraft:knockback_resistance", "amount": knockback, "id": id,
                       "operation": "add_value", "slot": slot }),
        );
    }
    Value::Array(v)
}

/// The components of a backtank's item (`BacktankItem`, a chestplate of copper or netherite;
/// no durability, so one to a stack), or None for another block. Its air is the native mod's.
pub fn backtank_item(name: &str) -> Option<Value> {
    Some(match name {
        // AllArmorMaterials.COPPER: 4 on the chest, enchantability 7.
        "copper_backtank" => json!({
            "minecraft:max_stack_size": 1,
            "minecraft:equippable": { "slot": "chest", "equip_sound": COPPER_EQUIP,
                                      "asset_id": "minecraft:copper" },
            "minecraft:attribute_modifiers": armor("chest", "chestplate", 4.0, 0.0, 0.0),
            "minecraft:enchantable": { "value": 7 }
        }),
        // ArmorMaterials.NETHERITE: 8, toughness 3, knockback resistance 0.1, enchantability 15;
        // `fireResistant()`.
        "netherite_backtank" => json!({
            "minecraft:max_stack_size": 1,
            "minecraft:equippable": { "slot": "chest",
                                      "equip_sound": "minecraft:item.armor.equip_netherite",
                                      "asset_id": "minecraft:netherite" },
            "minecraft:attribute_modifiers":
                armor("chest", "chestplate", 8.0, 3.0, f64::from(0.1f32)),
            "minecraft:enchantable": { "value": 15 },
            "minecraft:damage_resistant": { "types": "#minecraft:is_fire" }
        }),
        _ => return None,
    })
}

/// The declaration of an equipment item that is not a block, or None.
pub fn item(name: &str) -> Option<Value> {
    // DivingHelmetItem.getEnchantmentLevel/getAllEnchantments: Aqua Affinity always.
    let aqua = json!({ "minecraft:aqua_affinity": 1 });
    Some(match name {
        // COPPER_DIVING_HELMET: copper armour, `Type.HELMET.getDurability(7)` = 11 × 7.
        "copper_diving_helmet" => json!({
            "template": "minecraft:copper_helmet",
            "components": {
                "minecraft:max_damage": 77,
                "minecraft:enchantable": { "value": 7 },
                "minecraft:equippable": { "slot": "head", "equip_sound": COPPER_EQUIP,
                                          "asset_id": "minecraft:copper" },
                "minecraft:attribute_modifiers": armor("head", "helmet", 2.0, 0.0, 0.0),
                "minecraft:enchantments": aqua
            }
        }),
        // NETHERITE_DIVING_HELMET: netherite armour, `getDurability(37)` = 407, fire resistant:
        // the netherite helmet's numbers.
        "netherite_diving_helmet" => json!({
            "template": "minecraft:netherite_helmet",
            "components": { "minecraft:enchantments": aqua }
        }),
        // COPPER_DIVING_BOOTS: `Type.BOOTS.getDurability(7)` = 13 × 7.
        "copper_diving_boots" => json!({
            "template": "minecraft:copper_boots",
            "components": {
                "minecraft:max_damage": 91,
                "minecraft:enchantable": { "value": 7 },
                "minecraft:equippable": { "slot": "feet", "equip_sound": COPPER_EQUIP,
                                          "asset_id": "minecraft:copper" },
                "minecraft:attribute_modifiers": armor("feet", "boots", 1.0, 0.0, 0.0)
            }
        }),
        // NETHERITE_DIVING_BOOTS: `getDurability(37)` = 481, the netherite boots' numbers.
        "netherite_diving_boots" => json!({ "template": "minecraft:netherite_boots" }),
        // ExtendoGripItem: `durability(MAX_DAMAGE)` (200), uncommon.
        "extendo_grip" => json!({
            "template": "minecraft:stick",
            "max_stack_size": 1,
            "rarity": "uncommon",
            "components": { "minecraft:max_damage": 200 }
        }),
        // SandPaperItem: `durability(8)`, enchantability 1.
        "sand_paper" | "red_sand_paper" => json!({
            "template": "minecraft:stick",
            "max_stack_size": 1,
            "components": {
                "minecraft:max_damage": 8,
                "minecraft:enchantable": { "value": 1 }
            }
        }),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backtanks_are_chestplates_of_their_metal() {
        let copper = backtank_item("copper_backtank").unwrap();
        assert_eq!(copper["minecraft:equippable"]["slot"], "chest");
        assert_eq!(copper["minecraft:attribute_modifiers"][0]["amount"], 4.0);
        let netherite = backtank_item("netherite_backtank").unwrap();
        assert_eq!(netherite["minecraft:attribute_modifiers"][1]["amount"], 3.0);
        assert!(netherite.get("minecraft:damage_resistant").is_some());
        assert!(backtank_item("shaft").is_none());
    }

    #[test]
    fn copper_gear_wears_as_create_s_copper() {
        let helmet = item("copper_diving_helmet").unwrap();
        assert_eq!(helmet["components"]["minecraft:max_damage"], 77);
        let boots = item("copper_diving_boots").unwrap();
        assert_eq!(boots["components"]["minecraft:max_damage"], 91);
        assert_eq!(
            item("extendo_grip").unwrap()["components"]["minecraft:max_damage"],
            200
        );
        assert!(item("wrench").is_none());
    }
}
