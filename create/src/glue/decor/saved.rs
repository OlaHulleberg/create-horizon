//! A column's decorative state, saved as `create_decor`: its brackets (seven ints each: x, y, z,
//! wooden, facing, axis_along_first, type), placards' items and clipboards (`Pos` and `Item`
//! compounds), crates' supplies (`Pos` and the item's id) and hatches' filters (`Pos` and the filter's stack).

use super::super::server::ServerLevel;
use super::{DECOR, KEY, with};
use create_engine::decor::{Bracket, BracketKind};
use create_engine::kinetics::Dir;
use mod_api::{Compound, ItemStack, Tag};
use std::collections::HashMap;

fn pos_tag(p: [i32; 3]) -> Tag {
    Tag::IntArray(p.to_vec())
}

fn compound<const N: usize>(entries: [(&str, Tag); N]) -> Tag {
    Tag::Compound(Compound::from(entries.map(|(k, v)| (k.into(), v))))
}

/// The column `chunk`'s brackets, placards' items, crates' supplies, hatches' filters and
/// clipboards, as `create_decor`; none when it holds none.
pub(super) fn save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    let host = &*lv.host;
    let inside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] == chunk;
    let lists = DECOR.with_borrow(|all| {
        let d = all.get(&lv.dim)?;
        let mut brackets: Vec<_> = d.brackets.iter().filter(|(p, _)| inside(p)).collect();
        brackets.sort_by_key(|(p, _)| **p);
        let brackets: Vec<Tag> = brackets
            .into_iter()
            .map(|(p, b)| {
                Tag::IntArray(vec![
                    p[0],
                    p[1],
                    p[2],
                    i32::from(b.wooden),
                    b.facing as i32,
                    i32::from(b.along_first),
                    match b.kind {
                        BracketKind::Pipe => 0,
                        BracketKind::Cog => 1,
                        BracketKind::Shaft => 2,
                    },
                ])
            })
            .collect();
        let stacks = |m: &HashMap<[i32; 3], ItemStack>| -> Vec<Tag> {
            let mut v: Vec<_> = m.iter().filter(|(p, _)| inside(p)).collect();
            v.sort_by_key(|(p, _)| **p);
            v.into_iter()
                .filter_map(|(p, s)| {
                    let item = s.to_nbt(host)?;
                    Some(compound([
                        ("Pos", pos_tag(*p)),
                        ("Item", Tag::Compound(item)),
                    ]))
                })
                .collect()
        };
        let names = |m: &HashMap<[i32; 3], String>| -> Vec<Tag> {
            let mut v: Vec<_> = m.iter().filter(|(p, _)| inside(p)).collect();
            v.sort_by_key(|(p, _)| **p);
            v.into_iter()
                .map(|(p, s)| {
                    compound([
                        ("Pos", pos_tag(*p)),
                        ("Item", Tag::String(s.as_str().into())),
                    ])
                })
                .collect()
        };
        Some([
            ("brackets", brackets),
            ("placards", stacks(&d.placards)),
            ("clipboards", stacks(&d.clipboards)),
            ("supplies", names(&d.supplies)),
            ("hatches", {
                // A hatch's filter: its stack's NBT (an item id when saved before filter items).
                let mut v: Vec<_> = d.hatch_filters.iter().filter(|(p, _)| inside(p)).collect();
                v.sort_by_key(|(p, _)| **p);
                v.into_iter()
                    .filter_map(|(p, f)| {
                        let item = crate::glue::items::filters::filter_to_nbt(host, f)?;
                        Some(compound([("Pos", pos_tag(*p)), ("Item", item)]))
                    })
                    .collect()
            }),
        ])
    });
    match lists {
        Some(lists) if lists.iter().any(|(_, l)| !l.is_empty()) => {
            data.insert(
                KEY.into(),
                Tag::Compound(Compound::from(lists.map(|(k, l)| (k.into(), Tag::List(l))))),
            );
        }
        _ => {
            data.remove(&KEY.into());
        }
    }
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

/// A column joined a level: what it saved of ours back, each bracket and placard shown again.
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
