//! Create's packages (`PackageItem`, `PackageStyles`, Catnip's `Glob`): a box holding up to nine
//! stacks and an address, its style picked at random; the address rules every port and chain
//! junction applies (`matchAddress`); how a packager packs the inventory behind it
//! (`PackagerBlockEntity.attemptToSend`) and whether a package's contents fit an inventory
//! (`DefaultUnpackingHandler.unpack`, simulated). No game types: stacks are slot views.

/// `PackageItem.SLOTS`.
pub const SLOTS: usize = 9;

/// Where the glue keeps a package's address and contents in `minecraft:custom_data` (Horizon adds
/// no component types): `PACKAGE_ADDRESS` and `PACKAGE_CONTENTS` by their names.
pub const ADDRESS_KEY: &str = "create:package_address";
pub const CONTENTS_KEY: &str = "create:package_contents";

/// `PackageStyles.PackageStyle`: the item, its box's size in pixels and its rigging offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub item: &'static str,
    pub width: u8,
    pub height: u8,
    pub rigging: f32,
    pub rare: bool,
}

const fn cardboard(item: &'static str, width: u8, height: u8, rigging: f32) -> Style {
    Style {
        item,
        width,
        height,
        rigging,
        rare: false,
    }
}

/// `PackageStyles.rare(name)`: 12 by 10, rigging 21.
const fn rare(item: &'static str) -> Style {
    Style {
        item,
        width: 12,
        height: 10,
        rigging: 21.0,
        rare: true,
    }
}

/// `PackageStyles.STYLES`, in Create's order (`ALL_BOXES`): four cardboard boxes, then the rare.
pub const STYLES: [Style; 14] = [
    cardboard("create:cardboard_package_12x12", 12, 12, 23.0),
    cardboard("create:cardboard_package_10x12", 10, 12, 22.0),
    cardboard("create:cardboard_package_10x8", 10, 8, 18.0),
    cardboard("create:cardboard_package_12x10", 12, 10, 21.0),
    rare("create:rare_creeper_package"),
    rare("create:rare_darcy_package"),
    rare("create:rare_evan_package"),
    rare("create:rare_jinx_package"),
    rare("create:rare_kryppers_package"),
    rare("create:rare_simi_package"),
    rare("create:rare_starlotte_package"),
    rare("create:rare_thunder_package"),
    rare("create:rare_up_package"),
    rare("create:rare_vector_package"),
];

/// `PackageStyles.RARE_CHANCE`.
pub const RARE_CHANCE: u32 = 7500;

/// The style of a package item (`create:cardboard_package_12x12`), None for any other item.
pub fn style(item: &str) -> Option<&'static Style> {
    if !item.starts_with("create:") || !item.ends_with("package") && !item.contains("_package_") {
        return None;
    }
    STYLES.iter().find(|s| s.item == item)
}

/// `PackageItem.isPackage`.
pub fn is_package(item: &str) -> bool {
    style(item).is_some()
}

/// `PackageStyles.getRandomBox`: one in `RARE_CHANCE` from the rare boxes, else a cardboard
/// one; `next(n)` is `Random.nextInt(n)`.
pub fn random_box(mut next: impl FnMut(u32) -> u32) -> &'static str {
    let (from, len) = if next(RARE_CHANCE) == 0 {
        (4, 10)
    } else {
        (0, 4)
    };
    STYLES[from + next(len as u32) as usize % len].item
}

/// `PackageStyles.getDefaultBox`.
pub fn default_box() -> &'static str {
    STYLES[0].item
}

/// Java's `String.isBlank`: empty or only `Character.isWhitespace` characters.
pub fn is_blank(s: &str) -> bool {
    s.chars().all(java_whitespace)
}

fn java_whitespace(c: char) -> bool {
    match c {
        '\u{00A0}' | '\u{2007}' | '\u{202F}' | '\u{0085}' => false,
        '\u{001C}'..='\u{001F}' => true,
        c => c.is_whitespace(),
    }
}

/// `PackageItem.matchAddress(boxAddress, address)`: a blank address takes only blank boxes, `*`
/// on either side matches, else either side read as a glob (Catnip's `Glob.toRegexPattern`)
/// matching the other whole.
pub fn match_address(box_address: &str, address: &str) -> bool {
    if is_blank(address) {
        return is_blank(box_address);
    }
    if address == "*" || box_address == "*" || address == box_address {
        return true;
    }
    Glob::new(box_address).matches(address) || Glob::new(address).matches(box_address)
}

/// One element of a glob as Catnip turns it into a regex.
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Lit(char),
    /// `.`: any character but a line terminator.
    Any,
    /// `.*`.
    Star,
    /// `[...]`: ranges, negated with `!`.
    Set(bool, Vec<(char, char)>),
    /// `{a,b}`: `(?:a|b)`.
    Group(Vec<Vec<Tok>>),
    /// `{!a,b}` where the glob is anchored: `(?!a|b)` (Catnip adds `.*` after it).
    NotAhead(Vec<Vec<Tok>>),
    /// `{!a,b}` after a `*`: `(?<!a|b)`.
    NotBehind(Vec<Vec<Tok>>),
}

/// A glob compiled as Catnip's `Glob.toRegexPattern(glob, "")`: a glob it rejects becomes the
/// empty pattern, which matches only the empty string.
#[derive(Clone, Debug, PartialEq)]
pub struct Glob(Vec<Tok>);

impl Glob {
    pub fn new(glob: &str) -> Glob {
        Glob(parse(glob).unwrap_or_default())
    }

    /// `String.matches`: the whole text. A look-behind holding a `*` has no bounded length, which
    /// Java's regex rejects (Create would throw): no match here.
    pub fn matches(&self, text: &str) -> bool {
        let s: Vec<char> = text.chars().collect();
        go(&self.0, &s, 0, &mut |i| i == s.len())
    }
}

fn parse(glob: &str) -> Option<Vec<Tok>> {
    let g: Vec<char> = glob.chars().collect();
    let next = |i: usize| g.get(i).copied().unwrap_or('\0');
    let mut out: Vec<Tok> = Vec::new();
    // Inside `{...}`: the alternatives so far, and whether it is a negative look-around.
    let mut group: Option<(Vec<Vec<Tok>>, bool)> = None;
    let mut anchored = true;
    let mut negative_anchored = true;
    let mut i = 0;
    let push = |out: &mut Vec<Tok>, group: &mut Option<(Vec<Vec<Tok>>, bool)>, t: Tok| match group {
        Some((alts, _)) => alts.last_mut().unwrap().push(t),
        None => out.push(t),
    };
    while i < g.len() {
        let c = g[i];
        i += 1;
        match c {
            '*' => {
                push(&mut out, &mut group, Tok::Star);
                if group.is_none() {
                    anchored = false;
                }
            }
            '?' => {
                push(&mut out, &mut group, Tok::Any);
                if group.is_none() {
                    anchored = true;
                }
            }
            ',' => match &mut group {
                Some((alts, _)) => alts.push(Vec::new()),
                None => {
                    out.push(Tok::Lit(','));
                    anchored = true;
                }
            },
            '[' => {
                if next(i) == ']' || next(i) == '!' && next(i + 1) == ']' {
                    return None;
                }
                let mut negated = false;
                let mut items: Vec<(char, char)> = Vec::new();
                if next(i) == '^' {
                    items.push(('^', '^'));
                    i += 1;
                } else {
                    if next(i) == '!' {
                        negated = true;
                        i += 1;
                    }
                    if next(i) == '-' {
                        items.push(('-', '-'));
                        i += 1;
                    }
                }
                let mut range_start = false;
                let mut last = '\0';
                let mut c = c;
                while i < g.len() {
                    c = g[i];
                    i += 1;
                    if c == ']' {
                        break;
                    }
                    if c == '\\' {
                        if i == g.len() {
                            return None;
                        }
                        let e = next(i);
                        i += 1;
                        items.push((e, e));
                        continue;
                    }
                    if c == '-' {
                        if !range_start {
                            return None;
                        }
                        c = next(i);
                        i += 1;
                        if c == '\0' || c == ']' {
                            // `[a-]`: the dash is itself.
                            items.push(('-', '-'));
                            break;
                        }
                        if c < last {
                            return None;
                        }
                        items.pop();
                        items.push((last, c));
                        range_start = false;
                    } else {
                        items.push((c, c));
                        range_start = true;
                        last = c;
                    }
                }
                if c != ']' {
                    return None;
                }
                push(&mut out, &mut group, Tok::Set(negated, items));
                if group.is_none() {
                    anchored = true;
                }
            }
            '\\' => {
                if i == g.len() {
                    return None;
                }
                let n = g[i];
                i += 1;
                push(&mut out, &mut group, Tok::Lit(n));
                if group.is_none() {
                    anchored = true;
                }
            }
            '{' => {
                if group.is_some() {
                    return None;
                }
                let negative = next(i) == '!';
                if negative {
                    negative_anchored = anchored;
                    i += 1;
                }
                group = Some((vec![Vec::new()], negative));
            }
            '}' => match group.take() {
                Some((alts, negative)) => {
                    if !negative {
                        out.push(Tok::Group(alts));
                    } else if negative_anchored {
                        out.push(Tok::NotAhead(alts));
                        if anchored {
                            out.push(Tok::Star);
                            anchored = false;
                        }
                    } else {
                        out.push(Tok::NotBehind(alts));
                    }
                }
                None => {
                    out.push(Tok::Lit('}'));
                    anchored = true;
                }
            },
            c => {
                push(&mut out, &mut group, Tok::Lit(c));
                if group.is_none() {
                    anchored = true;
                }
            }
        }
    }
    if group.is_some() {
        return None;
    }
    Some(out)
}

/// Java's `.`: anything but a line terminator.
fn any(c: char) -> bool {
    !matches!(c, '\n' | '\r' | '\u{0085}' | '\u{2028}' | '\u{2029}')
}

/// Whether `toks` match `s` from `i`, ending where `k` accepts (a backtracking regex matcher).
fn go(toks: &[Tok], s: &[char], i: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
    let Some((t, rest)) = toks.split_first() else {
        return k(i);
    };
    match t {
        Tok::Lit(c) => s.get(i) == Some(c) && go(rest, s, i + 1, k),
        Tok::Any => s.get(i).is_some_and(|&c| any(c)) && go(rest, s, i + 1, k),
        Tok::Set(negated, items) => {
            s.get(i)
                .is_some_and(|&c| items.iter().any(|&(a, b)| (a..=b).contains(&c)) != *negated)
                && go(rest, s, i + 1, k)
        }
        Tok::Star => {
            let mut end = i;
            while end < s.len() && any(s[end]) {
                end += 1;
            }
            (i..=end).rev().any(|j| go(rest, s, j, k))
        }
        Tok::Group(alts) => alts
            .iter()
            .any(|alt| go(alt, s, i, &mut |j| go(rest, s, j, k))),
        Tok::NotAhead(alts) => {
            !alts.iter().any(|alt| go(alt, s, i, &mut |_| true)) && go(rest, s, i, k)
        }
        Tok::NotBehind(alts) => {
            if alts.iter().flatten().any(|t| *t == Tok::Star) {
                return false;
            }
            !alts
                .iter()
                .any(|alt| (0..=i).any(|j| go(alt, s, j, &mut |e| e == i)))
                && go(rest, s, i, k)
        }
    }
}

/// A slot of an inventory as packing sees it: its stack (compared by `K`), count, the item's
/// stack size, and whether it is bulky (`!canFitInsideContainerItems`: packages, shulker boxes).
#[derive(Clone, Debug, PartialEq)]
pub struct Slot<K> {
    pub key: K,
    pub count: u32,
    pub max: u32,
    pub bulky: bool,
    /// A package (`instanceof PackageItem`): sent as it is, not boxed.
    pub package: bool,
}

/// What `attemptToSend` (no request) takes: `(slot, count)` from the inventory in order, the
/// package's nine slots as `(the inventory slot the stack came from, count)`, and the slot of a
/// package taken whole (its box is sent as it is).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Packed {
    pub taken: Vec<(usize, u32)>,
    pub contents: [Option<(usize, u32)>; SLOTS],
    pub package: Option<usize>,
}

/// `PackagerBlockEntity.attemptToSend(null)`: nine passes over the inventory's slots, each
/// taking up to 64 (`extractItem(slot, 64)`) into the box's nine slots
/// (`insertItemStacked`: onto the same items, then empty slots); a bulky stack goes alone and
/// ends the packing. None when nothing was there.
pub fn pack<K: PartialEq>(slots: &[Option<Slot<K>>]) -> Option<Packed> {
    let mut left: Vec<u32> = slots
        .iter()
        .map(|s| s.as_ref().map_or(0, |s| s.count))
        .collect();
    let mut out = Packed::default();
    let mut any = false;
    'outer: for _ in 0..SLOTS {
        let mut moved = false;
        for (i, s) in slots.iter().enumerate() {
            let Some(s) = s else { continue };
            let extracted = left[i].min(64);
            if extracted == 0 {
                continue;
            }
            if s.bulky && any {
                continue;
            }
            any = true;
            let view: Vec<Option<(bool, u32)>> = out
                .contents
                .iter()
                .map(|c| {
                    c.map(|(from, n)| (slots[from].as_ref().is_some_and(|f| f.key == s.key), n))
                })
                .collect();
            let plan = crate::vault::insert_stacked(&view, extracted, s.max.clamp(1, 64));
            let mut transferred = 0;
            for (slot, n) in plan {
                transferred += n;
                match &mut out.contents[slot] {
                    Some((_, c)) => *c += n,
                    c => *c = Some((i, n)),
                }
            }
            if transferred > 0 {
                left[i] -= transferred;
                moved = true;
                match out.taken.iter_mut().find(|t| t.0 == i) {
                    Some(t) => t.1 += transferred,
                    None => out.taken.push((i, transferred)),
                }
            }
            if s.package {
                out.package = Some(i);
            }
            if s.bulky {
                break 'outer;
            }
        }
        // Another pass over the same inventory takes nothing more.
        if !moved {
            break;
        }
    }
    any.then_some(out)
}

/// A slot of the inventory a package is unpacked into: its stack, count and the slot's limit
/// (`getSlotLimit`), and whether the slot takes the item at all.
#[derive(Clone, Debug, PartialEq)]
pub struct Target<K> {
    pub stack: Option<(K, u32)>,
    pub limit: u32,
}

/// `DefaultUnpackingHandler.unpack` simulated: whether every stack of the box (`key`, count,
/// the item's stack size) finds room, slot by slot as Create counts it.
pub fn unpack_fits<K: PartialEq + Clone>(slots: &[Target<K>], items: &[(K, u32, u32)]) -> bool {
    let mut items: Vec<(K, u32, u32)> = items.to_vec();
    for slot in slots {
        // The stack in the slot as the simulation sees it, and what it added on top.
        let mut in_slot: Option<(K, u32, u32)> = slot.stack.as_ref().map(|(k, n)| {
            let max = items.iter().find(|i| i.0 == *k).map_or(64, |i| i.2);
            (k.clone(), *n, max)
        });
        let mut added = 0;
        for item in items.iter_mut() {
            if item.1 == 0 {
                continue;
            }
            // `insertItem(slot, toInsert, true)`: what the real slot would take.
            let room = match &slot.stack {
                None => slot.limit.min(item.2).min(item.1),
                Some((k, n)) if *k == item.0 => {
                    slot.limit.min(item.2).saturating_sub(*n).min(item.1)
                }
                Some(_) => 0,
            };
            if room == 0 {
                continue;
            }
            let Some((k, n, max)) = &in_slot else {
                // An empty slot: the stack goes in, up to the slot's limit.
                let put = item.1.min(slot.limit);
                in_slot = Some((item.0.clone(), put, item.2));
                item.1 -= put;
                continue;
            };
            if *k != item.0 {
                continue;
            }
            let limit = (*max).min(slot.limit);
            let insertable = item.1.min(limit.saturating_sub(n + added));
            let put = room.min(insertable);
            added += put;
            item.1 -= put;
        }
    }
    items.iter().all(|i| i.1 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_and_items() {
        assert_eq!(STYLES.len(), 14);
        assert!(is_package("create:cardboard_package_10x8"));
        assert!(is_package("create:rare_vector_package"));
        assert!(!is_package("create:cardboard"));
        assert!(!is_package("minecraft:chest"));
        assert_eq!(default_box(), "create:cardboard_package_12x12");
        assert_eq!(
            style("create:rare_up_package").map(|s| s.rigging),
            Some(21.0)
        );
        // One in 7,500 draws a rare box.
        let mut draws = [1u32, 2].into_iter();
        assert_eq!(random_box(|_| draws.next().unwrap()), STYLES[2].item);
        let mut draws = [0u32, 0].into_iter();
        assert_eq!(
            random_box(|n| if n == RARE_CHANCE {
                0
            } else {
                draws.next().unwrap()
            }),
            STYLES[4].item
        );
    }

    #[test]
    fn addresses_match_as_create_matches_them() {
        // Blank addresses take only blank boxes.
        assert!(match_address("", ""));
        assert!(match_address("  ", "\t"));
        assert!(!match_address("Home", ""));
        assert!(!match_address("", "Home"));
        // `*` on either side, equal strings.
        assert!(match_address("anything", "*"));
        assert!(match_address("*", "Base"));
        assert!(match_address("Base", "Base"));
        assert!(!match_address("Base", "base"), "case matters");
        // Either side as a glob.
        assert!(match_address("Farm*", "Farm North"));
        assert!(match_address("Farm North", "Farm*"));
        assert!(match_address("Farm ?", "Farm 1"));
        assert!(!match_address("Farm ?", "Farm 12"));
        assert!(match_address("{Farm,Mine} *", "Mine 3"));
        assert!(!match_address("{Farm,Mine} *", "Mill 3"));
        assert!(match_address("Storage[0-9]", "Storage7"));
        assert!(!match_address("Storage[!0-9]", "Storage7"));
        assert!(match_address("Storage[!0-9]", "StorageX"));
        // Regex characters are literal.
        assert!(match_address("a.b", "a.b"));
        assert!(!match_address("a.b", "axb"));
        assert!(match_address("(x)+", "(x)+"));
        // Escapes.
        assert!(match_address("\\*", "*x") == match_address("*x", "\\*"));
        assert!(Glob::new("a\\*").matches("a*"));
        assert!(!Glob::new("a\\*").matches("ab"));
        // Negative groups: anchored, a look-ahead (anything not starting so); after a `*`, a
        // look-behind (not ending so).
        assert!(Glob::new("{!Farm}").matches("Mine"));
        assert!(!Glob::new("{!Farm}").matches("Farmhouse"));
        assert!(Glob::new("*{!.txt}").matches("notes.md"));
        assert!(!Glob::new("*{!.txt}").matches("notes.txt"));
        // A glob Catnip rejects is the empty pattern.
        assert!(!Glob::new("[]").matches("x"));
        assert!(Glob::new("{a").matches(""));
        assert!(!Glob::new("[b-a]").matches("a"));
        // `.` stops at line ends.
        assert!(!Glob::new("a*").matches("a\nb"));
        // A dash at the end of a set is itself.
        assert!(Glob::new("[a-]").matches("-"));
        assert!(Glob::new("x,y").matches("x,y"));
    }

    fn slot(key: &'static str, count: u32) -> Option<Slot<&'static str>> {
        Some(Slot {
            key,
            count,
            max: if key == "egg" { 16 } else { 64 },
            bulky: key == "package" || key == "shulker",
            package: key == "package",
        })
    }

    #[test]
    fn packing_takes_up_to_nine_stacks_in_slot_order() {
        // 12 slots of 64 cobblestone: the first nine fill the box.
        let inv: Vec<_> = (0..12).map(|_| slot("cobble", 64)).collect();
        let p = pack(&inv).unwrap();
        assert_eq!(p.taken, (0..9).map(|i| (i, 64)).collect::<Vec<_>>());
        assert!(
            p.contents
                .iter()
                .enumerate()
                .all(|(i, c)| *c == Some((i, 64)))
        );
        // Partial stacks merge: 10 + 10 iron become one stack of 20.
        let inv = vec![slot("iron", 10), None, slot("gold", 5), slot("iron", 10)];
        let p = pack(&inv).unwrap();
        assert_eq!(p.taken, vec![(0, 10), (2, 5), (3, 10)]);
        assert_eq!(p.contents[0], Some((0, 20)));
        assert_eq!(p.contents[1], Some((2, 5)));
        assert_eq!(p.contents[2], None);
        // Eggs stack to 16: 40 eggs take three slots (16, 16, 8).
        let p = pack(&[slot("egg", 40)]).unwrap();
        assert_eq!(
            &p.contents[..3],
            &[Some((0, 16)), Some((0, 16)), Some((0, 8))]
        );
        // A full box leaves the rest: 9 × 64 of one and more of another.
        let mut inv: Vec<_> = (0..9).map(|_| slot("cobble", 64)).collect();
        inv.push(slot("dirt", 3));
        let p = pack(&inv).unwrap();
        assert!(p.taken.iter().all(|t| t.0 < 9));
        // Nothing there.
        assert_eq!(pack::<&str>(&[None, None]), None);
    }

    #[test]
    fn a_package_goes_alone() {
        // A package first: sent as it is.
        let p = pack(&[slot("package", 1), slot("iron", 4)]).unwrap();
        assert_eq!(p.package, Some(0));
        assert_eq!(p.taken, vec![(0, 1)]);
        // Items first: the package is skipped and stays.
        let p = pack(&[slot("iron", 4), slot("package", 1)]).unwrap();
        assert_eq!(p.package, None);
        assert_eq!(p.taken, vec![(0, 4)]);
        // A shulker box goes alone, boxed.
        let p = pack(&[slot("shulker", 1), slot("iron", 4)]).unwrap();
        assert_eq!((p.package, p.contents[0]), (None, Some((0, 1))));
    }

    fn target(stack: Option<(&'static str, u32)>) -> Target<&'static str> {
        Target { stack, limit: 64 }
    }

    #[test]
    fn unpacking_fits_only_when_every_stack_has_room() {
        let items = [("iron", 64, 64), ("iron", 20, 64), ("gold", 5, 64)];
        // Three empty slots: iron 64, iron 20 (no room left on the full one), gold 5.
        assert!(unpack_fits(
            &[target(None), target(None), target(None)],
            &items
        ));
        // Two: the 20 iron has nowhere to go.
        assert!(!unpack_fits(&[target(None), target(None)], &items));
        // A slot holding 44 iron takes the 20.
        assert!(unpack_fits(
            &[target(None), target(Some(("iron", 44))), target(None)],
            &items
        ));
        // ... but not when it holds 50.
        assert!(!unpack_fits(
            &[target(None), target(Some(("iron", 50))), target(None)],
            &items
        ));
        // Another item in the way.
        assert!(!unpack_fits(
            &[target(Some(("dirt", 1)))],
            &[("gold", 1, 64)]
        ));
        // Eggs stack to 16 in an empty slot taking two stacks of 10 (Create's simulation counts
        // the item's stack size for the second).
        assert!(!unpack_fits(
            &[target(None)],
            &[("egg", 10, 16), ("egg", 10, 16)]
        ));
        assert!(unpack_fits(
            &[target(None)],
            &[("egg", 10, 16), ("egg", 6, 16)]
        ));
    }
}
