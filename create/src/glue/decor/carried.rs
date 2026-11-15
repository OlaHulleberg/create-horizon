//! What decor keeps for a block that rides a moving structure (`Subsystem::carry_out`,
//! Create's block entity NBT): a shaft's, cog's or pipe's bracket (`BracketedBlockEntityBehaviour`,
//! its facing turned with the structure as `BracketedKineticBlockEntity.transform` turns it), a
//! placard's item, a placed clipboard (its pages), a creative crate's supply and an item hatch's
//! filter. Taken before the block leaves, so its removal drops none of them.

use super::with;
use crate::glue::server::ServerLevel;
use create_engine::Block;
use create_engine::contraption::Transform;
use create_engine::decor::{Bracket, BracketKind};
use create_engine::kinetics::Dir;
use mod_api::{Compound, ItemStack, Tag};

const BRACKET: &str = "Bracket";
const PLACARD: &str = "PlacardItem";
const CLIPBOARD: &str = "ClipboardItem";
const SUPPLY: &str = "CrateSupply";
const HATCH: &str = "HatchFilter";

pub(super) fn carry_out(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block, out: &mut Compound) {
    let host = &*lv.host;
    let (bracket, placard, clipboard, supply, hatch) = with(lv.dim, |d| {
        (
            d.brackets.remove(&pos),
            d.placards.remove(&pos),
            d.clipboards.remove(&pos),
            (block == Block::CreativeCrate)
                .then(|| d.supplies.remove(&pos))
                .flatten(),
            (block == Block::ItemHatch)
                .then(|| d.hatch_filters.remove(&pos))
                .flatten(),
        )
    });
    if let Some(b) = bracket {
        let kind = match b.kind {
            BracketKind::Pipe => 0,
            BracketKind::Cog => 1,
            BracketKind::Shaft => 2,
        };
        out.insert(
            BRACKET.into(),
            Tag::IntArray(vec![
                i32::from(b.wooden),
                b.facing as i32,
                i32::from(b.along_first),
                kind,
            ]),
        );
    }
    let stack = |s: Option<ItemStack>| s.and_then(|s| s.to_nbt(host)).map(Tag::Compound);
    if let Some(t) = stack(placard) {
        out.insert(PLACARD.into(), t);
    }
    if let Some(t) = stack(clipboard) {
        out.insert(CLIPBOARD.into(), t);
    }
    if let Some(s) = supply {
        out.insert(SUPPLY.into(), Tag::String(s.as_str().into()));
    }
    if let Some(t) = hatch.and_then(|f| crate::glue::items::filters::filter_to_nbt(host, &f)) {
        out.insert(HATCH.into(), t);
    }
}

pub(super) fn carry_in(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    data: &Compound,
    turn: Transform,
) {
    let host = &*lv.host;
    let bracket = match data.get(&BRACKET.into()) {
        Some(Tag::IntArray(v)) if v.len() == 4 => Some(Bracket {
            wooden: v[0] != 0,
            facing: turn.dir(Dir::ALL[v[1].clamp(0, 5) as usize]),
            along_first: v[2] != 0,
            kind: [BracketKind::Pipe, BracketKind::Cog, BracketKind::Shaft]
                [v[3].clamp(0, 2) as usize],
        }),
        _ => None,
    };
    let stack = |k: &str| match data.get(&k.into()) {
        Some(Tag::Compound(c)) => ItemStack::from_nbt(host, c),
        _ => None,
    };
    let text = |k: &str| match data.get(&k.into()) {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    };
    let (placard, clipboard) = (stack(PLACARD), stack(CLIPBOARD));
    let supply = text(SUPPLY);
    let hatch = data
        .get(&HATCH.into())
        .and_then(|t| crate::glue::items::filters::filter_from_nbt(host, t));
    with(lv.dim, |d| {
        if let Some(b) = bracket {
            d.brackets.insert(pos, b);
            d.shown.push(pos);
        }
        if let Some(s) = placard.filter(|_| block == Block::Placard) {
            d.placards.insert(pos, s);
            d.shown.push(pos);
        }
        if let Some(s) = clipboard.filter(|_| block == Block::Clipboard) {
            d.clipboards.insert(pos, s);
        }
        if let Some(s) = supply.filter(|_| block == Block::CreativeCrate) {
            d.supplies.insert(pos, s);
        }
        if let Some(s) = hatch.filter(|_| block == Block::ItemHatch) {
            d.hatch_filters.insert(pos, s);
            d.shown.push(pos);
        }
    });
}
