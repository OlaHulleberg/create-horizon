//! Block entity fields from MIT glue `Subsystem::save_entities` / `load_entity`: what each
//! entity's `saveAdditional` writes, published with its column (`entity_data`).
use super::codec::{self, Compound, Tag};
use crate::scenario::Pos;
use create_engine::Block;
use create_engine::world::CreateWorld;
use std::collections::BTreeMap;

pub fn save_column(w: &CreateWorld, column: (i32, i32), now: u64) -> BTreeMap<Pos, Compound> {
    let (cx, cz) = column;
    let mut out: BTreeMap<Pos, Compound> = BTreeMap::new();
    for (p, rpm) in w.column_settings(0, cx, cz) {
        out.entry(p)
            .or_default()
            .insert("ScrollValue".into(), Tag::Int(rpm));
    }
    for (c, inv) in w.column_belts(0, cx, cz) {
        out.entry(c)
            .or_default()
            .insert("Inventory".into(), Tag::Compound(belt_to_tag(c, inv)));
    }
    for (p, b) in w.column_brass(0, cx, cz) {
        out.entry(p)
            .or_default()
            .insert("BrassTunnel".into(), Tag::Compound(brass_to_tag(p, b)));
    }
    for (p, at) in w.column_chutes(0, cx, cz) {
        out.entry(p)
            .or_default()
            .insert("ItemPosition".into(), Tag::Float(at.to_bits()));
    }
    for (p, cooldown) in w.column_funnels(0, cx, cz, now) {
        out.entry(p)
            .or_default()
            .insert("TransferCooldown".into(), Tag::Int(cooldown as i32));
    }
    super::fluids::save_entities(w, column, &mut out);
    out
}

pub fn load(w: &mut CreateWorld, p: Pos, block: Block, tag: &Compound, now: u64) {
    super::fluids::load(w, p, block, tag);
    match block {
        Block::CreativeMotor | Block::RotationSpeedController => {
            if let Some(Tag::Int(rpm)) = tag.get("ScrollValue") {
                w.load_settings(0, std::iter::once((p, *rpm)));
            }
        }
        Block::Belt => {
            if let Some(inv) = belt_inventory(p, tag) {
                w.load_belt_items(0, std::iter::once((p, inv)));
            }
        }
        Block::BrassTunnel => {
            if let Some(Tag::Compound(c)) = tag.get("BrassTunnel") {
                let mut c = c.clone();
                c.insert("Pos".into(), Tag::IntArray(p.to_vec()));
                let data = Compound::from([("brass".into(), Tag::List(vec![Tag::Compound(c)]))]);
                w.load_brass(0, codec::brass_from_nbt(&data).into_iter());
            }
        }
        Block::Chute | Block::SmartChute => {
            if let Some(Tag::Float(bits)) = tag.get("ItemPosition") {
                w.load_chutes(0, std::iter::once((p, f32::from_bits(*bits))));
            }
        }
        Block::AndesiteFunnel
        | Block::AndesiteBeltFunnel
        | Block::BrassFunnel
        | Block::BrassBeltFunnel => {
            if let Some(Tag::Int(cooldown)) = tag.get("TransferCooldown") {
                let cooldown = (*cooldown).max(0) as u32;
                w.load_funnels(0, now, std::iter::once((p, cooldown)));
            }
        }
        _ => {}
    }
}

/// The belt controller's `Inventory` as `load` reads it.
pub fn belt_inventory(c: Pos, tag: &Compound) -> Option<create_engine::belt::Inventory> {
    let Some(Tag::Compound(b)) = tag.get("Inventory") else {
        return None;
    };
    let mut b = b.clone();
    b.insert("pos".into(), Tag::IntArray(c.to_vec()));
    let data = Compound::from([("belts".into(), Tag::List(vec![Tag::Compound(b)]))]);
    codec::belts_from_nbt(&data).pop().map(|(_, inv)| inv)
}

fn belt_to_tag(c: Pos, inv: create_engine::belt::Inventory) -> Compound {
    let Tag::List(mut list) = codec::belts_to_nbt(&[(c, inv)]) else {
        unreachable!()
    };
    let Some(Tag::Compound(mut b)) = list.pop() else {
        unreachable!()
    };
    b.remove("pos");
    b
}

fn brass_to_tag(p: Pos, saved: create_engine::tunnel::SavedBrass) -> Compound {
    let Tag::List(mut list) = codec::brass_to_nbt(&[(p, saved)]) else {
        unreachable!()
    };
    let Some(Tag::Compound(mut c)) = list.pop() else {
        unreachable!()
    };
    c.remove("Pos");
    c
}
