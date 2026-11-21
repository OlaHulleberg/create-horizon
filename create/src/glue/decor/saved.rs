//! What decorative blocks keep in their block entities: brackets, placards' items, clipboards,
//! creative crates' supplies and hatches' filters. An older version kept them with the column
//! as `create_decor` (brackets as seven ints each: x, y, z, wooden, facing, axis_along_first,
//! type; the rest as `Pos` and `Item` compounds), read once by [`load`].

use super::super::entity_data::Entities;
use super::super::server::ServerLevel;
use super::{DECOR, KEY, with};
use create_engine::Block;
use create_engine::decor::{Bracket, BracketKind};
use create_engine::kinetics::Dir;
use mod_api::{Compound, ItemStack, Tag};

const BRACKET: &str = "BracketState";
const PLACARD: &str = "PlacardItem";
const CLIPBOARD: &str = "ClipboardItem";
const SUPPLY: &str = "Supply";
const HATCH: &str = "HatchFilter";

pub(super) fn saved_keys(block: Block) -> &'static [&'static str] {
    match block {
        Block::Placard => &[PLACARD],
        Block::Clipboard => &[CLIPBOARD],
        Block::CreativeCrate => &[SUPPLY],
        Block::ItemHatch => &[HATCH],
        _ => &[BRACKET],
    }
}

fn bracket_kind(kind: BracketKind) -> i32 {
    match kind {
        BracketKind::Pipe => 0,
        BracketKind::Cog => 1,
        BracketKind::Shaft => 2,
    }
}

/// `BracketedBlockEntityBehaviour.write`, `PlacardBlockEntity.write`,
/// `ClipboardBlockEntity.write`, `CreativeCrateBlockEntity`'s filter and the hatch's
/// `FilteringBehaviour.write` of column `chunk`.
pub(super) fn save(lv: &ServerLevel<'_>, chunk: [i32; 2], out: &mut Entities) {
    let host = &*lv.host;
    let inside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] == chunk;
    DECOR.with_borrow(|all| {
        let Some(d) = all.get(&lv.dim) else {
            return;
        };
        for (p, b) in d.brackets.iter().filter(|(p, _)| inside(p)) {
            let c = Compound::from([
                ("Wooden".into(), Tag::Byte(i8::from(b.wooden))),
                ("Facing".into(), Tag::Int(b.facing as i32)),
                ("AlongFirst".into(), Tag::Byte(i8::from(b.along_first))),
                ("Kind".into(), Tag::Int(bracket_kind(b.kind))),
            ]);
            out.entry(*p)
                .or_default()
                .insert(BRACKET.into(), Tag::Compound(c));
        }
        for (key, stacks) in [(PLACARD, &d.placards), (CLIPBOARD, &d.clipboards)] {
            for (p, stack) in stacks.iter().filter(|(p, _)| inside(p)) {
                if let Some(item) = stack.to_nbt(host) {
                    out.entry(*p)
                        .or_default()
                        .insert(key.into(), Tag::Compound(item));
                }
            }
        }
        for (p, item) in d.supplies.iter().filter(|(p, _)| inside(p)) {
            out.entry(*p)
                .or_default()
                .insert(SUPPLY.into(), Tag::String(item.as_str().into()));
        }
        for (p, f) in d.hatch_filters.iter().filter(|(p, _)| inside(p)) {
            if let Some(item) = crate::glue::items::filters::filter_to_nbt(host, f) {
                out.entry(*p).or_default().insert(HATCH.into(), item);
            }
        }
    });
}

/// The `read`s of the fields [`save`] writes.
pub(super) fn load_entity(lv: &mut ServerLevel<'_>, pos: [i32; 3], block: Block, tag: &Compound) {
    let stack = |k: &str| match tag.get(&k.into()) {
        Some(Tag::Compound(item)) => ItemStack::from_nbt(&*lv.host, item),
        _ => None,
    };
    let (placard, clipboard) = (stack(PLACARD), stack(CLIPBOARD));
    let hatch = tag
        .get(&HATCH.into())
        .and_then(|t| crate::glue::items::filters::filter_from_nbt(&*lv.host, t));
    with(lv.dim, |d| match block {
        Block::Placard => {
            match placard {
                Some(s) => d.placards.insert(pos, s),
                None => d.placards.remove(&pos),
            };
            d.shown.push(pos);
        }
        Block::Clipboard => {
            match clipboard {
                Some(s) => d.clipboards.insert(pos, s),
                None => d.clipboards.remove(&pos),
            };
        }
        Block::CreativeCrate => {
            match tag.get(&SUPPLY.into()) {
                Some(Tag::String(item)) => d.supplies.insert(pos, item.to_string_lossy()),
                _ => d.supplies.remove(&pos),
            };
        }
        Block::ItemHatch => {
            match hatch {
                Some(f) => d.hatch_filters.insert(pos, f),
                None => d.hatch_filters.remove(&pos),
            };
            d.shown.push(pos);
        }
        _ => {
            let Some(Tag::Compound(c)) = tag.get(&BRACKET.into()) else {
                if d.brackets.remove(&pos).is_some() {
                    d.shown.push(pos);
                }
                return;
            };
            let int = |k: &str| match c.get(&k.into()) {
                Some(Tag::Int(v)) => *v,
                _ => 0,
            };
            let flag = |k: &str| matches!(c.get(&k.into()), Some(Tag::Byte(b)) if *b != 0);
            let bracket = Bracket {
                wooden: flag("Wooden"),
                facing: Dir::ALL[int("Facing").clamp(0, 5) as usize],
                along_first: flag("AlongFirst"),
                kind: [BracketKind::Pipe, BracketKind::Cog, BracketKind::Shaft]
                    [int("Kind").clamp(0, 2) as usize],
            };
            d.brackets.insert(pos, bracket);
            d.shown.push(pos);
        }
    });
}

fn list(data: &Compound, key: &str) -> Vec<Tag> {
    match data.get(&key.into()) {
        Some(Tag::List(l)) => l.to_vec(),
        _ => Vec::new(),
    }
}

fn pos_of(c: &Compound) -> Option<[i32; 3]> {
    match c.get(&"Pos".into()) {
        Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
        _ => None,
    }
}

/// The `create_decor` an older version saved with the column, back, each bracket and placard
/// shown again.
pub(super) fn load(lv: &mut ServerLevel<'_>, data: &Compound) {
    let Some(Tag::Compound(data)) = data.get(&KEY.into()) else {
        return;
    };
    let mut placed = Vec::new();
    for t in list(data, "brackets") {
        let Tag::IntArray(v) = t else { continue };
        if v.len() != 7 {
            continue;
        }
        let kind =
            [BracketKind::Pipe, BracketKind::Cog, BracketKind::Shaft][v[6].clamp(0, 2) as usize];
        let bracket = Bracket {
            wooden: v[3] != 0,
            facing: Dir::ALL[v[4].clamp(0, 5) as usize],
            along_first: v[5] != 0,
            kind,
        };
        placed.push(([v[0], v[1], v[2]], bracket));
    }
    let mut stacks = Vec::new();
    for (key, placard) in [("placards", true), ("clipboards", false)] {
        for t in list(data, key) {
            let Tag::Compound(c) = t else { continue };
            let Some(p) = pos_of(&c) else { continue };
            let Some(Tag::Compound(item)) = c.get(&"Item".into()) else {
                continue;
            };
            if let Some(s) = ItemStack::from_nbt(&*lv.host, item) {
                stacks.push((p, placard, s));
            }
        }
    }
    let hatches: Vec<_> = list(data, "hatches")
        .into_iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let f =
                crate::glue::items::filters::filter_from_nbt(&*lv.host, c.get(&"Item".into())?)?;
            Some((pos_of(&c)?, f))
        })
        .collect();
    with(lv.dim, |d| {
        for (p, b) in placed {
            d.brackets.insert(p, b);
            d.shown.push(p);
        }
        for (p, placard, s) in stacks {
            if placard {
                d.placards.insert(p, s);
                d.shown.push(p);
            } else {
                d.clipboards.insert(p, s);
            }
        }
        for t in list(data, "supplies") {
            let Tag::Compound(c) = t else { continue };
            let item = match c.get(&"Item".into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => continue,
            };
            let Some(p) = pos_of(&c) else { continue };
            d.supplies.insert(p, item);
        }
        for (p, f) in hatches {
            d.hatch_filters.insert(p, f);
            d.shown.push(p);
        }
    });
}
