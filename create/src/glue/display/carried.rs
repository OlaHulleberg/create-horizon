//! What the displays keep for a block that rides a moving structure (`Subsystem::carry_out`,
//! Create's block entity NBT): a nixie tube's custom text (`NixieTubeBlockEntity.write`:
//! `CustomText`, `CustomTextIndex`), and a display link's target, line, source and its
//! configuration (`DisplayLinkBlockEntity.write`: a station summary's settings and the gauges
//! joined to it too), the target's offset turned with the structure
//! (`DisplayLinkBlockEntity.transform`), and a display board's lines (`FlapDisplayBlockEntity
//! .write`: a controller's size, lines, colours and glow). Each waits where it goes back as a
//! saved one does (`displays_from_nbt`), and is kept as it is placed there.

use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::contraption::Transform;
use create_engine::display::{Config, Link, Source, is_nixie};
use mod_api::{Compound, Tag};

const NIXIE: &str = "NixieText";
const NIXIE_INDEX: &str = "NixieIndex";
/// A tube's two characters as it showed them (`NixieTubeRenderer`), for drawing it moving.
pub(crate) const SHOWN: &str = "NixieShown";
const LINK: &str = "DisplayLink";
const LINK_TEXTS: &str = "DisplayLinkTexts";
/// A link's station summary settings and its joined gauges (`LinkedGauges`, as Create keeps
/// them: not turned), when it has any.
const LINK_MORE: &str = "DisplayLinkMore";
/// A controlling display board's lines, as a column saves them (`board_to_nums`).
pub(crate) const BOARD: &str = "DisplayBoard";
pub(crate) const BOARD_TEXTS: &str = "DisplayBoardTexts";

pub(super) fn carry_out(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block, out: &mut Compound) {
    let dim = lv.dim;
    with_world(|w| {
        let d = &mut w.dims[dim].displays;
        if is_nixie(block) {
            if let Some(n) = d.nixies.remove(&pos) {
                // Its clients forget it where it was (`nixie_removed`).
                d.forgotten.push(pos);
                // What it shows, for the moving structure's clients to draw.
                let (left, right) = n.displayed();
                out.insert(
                    SHOWN.into(),
                    Tag::List(vec![
                        Tag::String(left.as_str().into()),
                        Tag::String(right.as_str().into()),
                    ]),
                );
                if let Some(text) = n.custom {
                    out.insert(NIXIE.into(), Tag::String(text.as_str().into()));
                    out.insert(NIXIE_INDEX.into(), Tag::Int(n.index));
                }
            }
        } else if block == Block::DisplayBoard {
            if let Some(mut b) = d.boards.remove(&pos) {
                d.forgotten.push(pos);
                if b.controller {
                    let (mut nums, mut texts) = (Vec::new(), Vec::new());
                    super::board_to_nums(&mut b, &mut nums, &mut texts);
                    out.insert(BOARD.into(), Tag::IntArray(nums));
                    out.insert(BOARD_TEXTS.into(), Tag::List(texts));
                }
            }
        } else if block == Block::DisplayLink
            && let Some(l) = d.links.remove(&pos)
        {
            let c = &l.config;
            let start = c.start_time.unwrap_or(0);
            let mut nums = l.target_offset.to_vec();
            nums.extend([
                l.target_line,
                c.directional,
                c.mode,
                c.cycle,
                i32::from(c.start_time.is_some()),
                (start >> 32) as i32,
                start as i32,
                c.format,
                c.collected,
                c.interval,
            ]);
            out.insert(LINK.into(), Tag::IntArray(nums));
            out.insert(
                LINK_TEXTS.into(),
                Tag::List(vec![
                    Tag::String(l.source.map_or("", Source::id).into()),
                    Tag::String(c.label.as_str().into()),
                ]),
            );
            let mut more = Compound::new();
            if let Some(f) = &c.filter {
                more.insert("Filter".into(), Tag::String(f.as_str().into()));
            }
            if let Some(n) = c.name_column {
                more.insert("NameColumn".into(), Tag::Int(n));
            }
            if let Some(n) = c.platform_column {
                more.insert("PlatformColumn".into(), Tag::Int(n));
            }
            if !l.panels.is_empty() {
                let gauges = l.panels.iter();
                let gauges = gauges.flat_map(|(q, slot)| [q[0], q[1], q[2], i32::from(*slot)]);
                more.insert("LinkedGauges".into(), Tag::IntArray(gauges.collect()));
            }
            if !more.is_empty() {
                out.insert(LINK_MORE.into(), Tag::Compound(more));
            }
        }
    });
}

pub(super) fn carry_in(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    data: &Compound,
    turn: Transform,
) {
    let dim = lv.dim;
    if is_nixie(block) {
        let Some(Tag::String(text)) = data.get(&NIXIE.into()) else {
            return;
        };
        let index = match data.get(&NIXIE_INDEX.into()) {
            Some(Tag::Int(i)) => *i,
            _ => 0,
        };
        with_world(|w| {
            let d = &mut w.dims[dim].displays;
            let n = d.nixies.entry(pos).or_default();
            n.custom = Some(text.to_string_lossy());
            n.index = index;
            d.shown.push(pos);
        });
        return;
    }
    if block == Block::DisplayBoard {
        if let Some(b) = board_of(data) {
            with_world(|w| {
                let d = &mut w.dims[dim].displays;
                d.boards.insert(pos, b);
                // Joined again with the boards beside it where it went back (`updateColumn`).
                d.board_checks.push(pos);
                d.shown.push(pos);
            });
        }
        return;
    }
    if block != Block::DisplayLink {
        return;
    }
    let (Some(Tag::IntArray(v)), Some(Tag::List(texts))) =
        (data.get(&LINK.into()), data.get(&LINK_TEXTS.into()))
    else {
        return;
    };
    let [
        ox,
        oy,
        oz,
        line,
        directional,
        mode,
        cycle,
        has_start,
        hi,
        lo,
        format,
        collected,
        interval,
    ] = v[..]
    else {
        return;
    };
    let text = |i: usize| match texts.get(i) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let start = (i64::from(hi) << 32) | i64::from(lo as u32);
    let more = match data.get(&LINK_MORE.into()) {
        Some(Tag::Compound(c)) => c.clone(),
        _ => Compound::new(),
    };
    let int = |k: &str| match more.get(&k.into()) {
        Some(Tag::Int(n)) => Some(*n),
        _ => None,
    };
    let panels = match more.get(&"LinkedGauges".into()) {
        Some(Tag::IntArray(g)) => g
            .as_chunks::<4>()
            .0
            .iter()
            .map(|v| ([v[0], v[1], v[2]], v[3].clamp(0, 3) as u8))
            .collect(),
        _ => Vec::new(),
    };
    let link = Link {
        target_offset: turn.vec([ox, oy, oz]),
        target_line: line,
        source: Source::from_id(&text(0)),
        config: Config {
            label: text(1),
            directional,
            mode,
            cycle,
            start_time: (has_start != 0).then_some(start),
            format,
            collected,
            interval,
            filter: match more.get(&"Filter".into()) {
                Some(Tag::String(f)) => Some(f.to_string_lossy()),
                _ => None,
            },
            name_column: int("NameColumn"),
            platform_column: int("PlatformColumn"),
            ..Config::default()
        },
        due: None,
        last: None,
        panels,
    };
    with_world(|w| w.dims[dim].displays.links.insert(pos, link));
}

/// The controlling board [`carry_out`] wrote into `data`, if it was one.
pub(crate) fn board_of(data: &Compound) -> Option<create_engine::display::Board> {
    let (Some(Tag::IntArray(nums)), Some(Tag::List(texts))) =
        (data.get(&BOARD.into()), data.get(&BOARD_TEXTS.into()))
    else {
        return None;
    };
    let mut nums = nums.iter().copied();
    let mut texts = texts.iter().map(|t| match t {
        Tag::String(s) => s.to_string_lossy(),
        _ => String::new(),
    });
    super::board_from_nums(&mut || nums.next(), &mut || texts.next())
}
