//! Explicit surroundings shared with TransportOracle's authored Java fixture.
use super::{Answer, axis, f, n};
use create_engine::{
    belt, belt_connector,
    kinetics::{Axis, Dir},
};

struct Ground<'a> {
    v: &'a [&'a str],
}
impl belt_connector::Ground for Ground<'_> {
    fn loaded(&self, pos: [i32; 3]) -> bool {
        let bit = if pos == [0; 3] { 1 } else { 2 };
        n(self.v, 8) & bit == 0
    }
    fn shaft(&self, pos: [i32; 3]) -> Option<Axis> {
        let second = [n(self.v, 3), n(self.v, 4), n(self.v, 5)];
        if pos == [0; 3] {
            (n(self.v, 8) & 4 == 0).then(|| axis(n(self.v, 1) as usize))
        } else if pos == second {
            (n(self.v, 8) & 8 == 0).then(|| axis(n(self.v, 2) as usize))
        } else {
            match n(self.v, 9) {
                1 => Some(axis(n(self.v, 1) as usize)),
                2 => Some(axis((n(self.v, 1) as usize + 1) % 3)),
                _ => None,
            }
        }
    }
    fn speed(&self, pos: [i32; 3]) -> f32 {
        f(self.v, if pos == [0; 3] { 6 } else { 7 })
    }
    fn replaceable(&self, _: [i32; 3]) -> bool {
        n(self.v, 9) == 0
    }
}

struct Effects {
    ending: belt::Ending,
    room: u8,
    segment: i32,
    mode: i32,
}
impl belt::Surroundings for Effects {
    fn ending(&mut self) -> belt::Ending {
        self.ending
    }
    fn insert(&mut self, item: &belt::Item) -> Option<u8> {
        let taken = item.count.min(self.room);
        self.room -= taken;
        Some(item.count - taken)
    }
    fn process(&mut self, segment: i32, _: &mut belt::Item, _: bool) -> Option<belt::Process> {
        if self.mode == 0 || segment != self.segment {
            return None;
        }
        Some(match self.mode {
            1 => belt::Process::Hold,
            3 => belt::Process::Remove,
            _ => belt::Process::Pass,
        })
    }
}

pub fn probe(v: &[&str]) -> Answer {
    match v[0] {
        "C" => {
            if n(v, 10) != belt_connector::MAX_LENGTH {
                return Err("fixture requests a non-default maximum belt length");
            }
            let g = Ground { v };
            let end = [n(v, 3), n(v, 4), n(v, 5)];
            if !belt_connector::valid_axis(&g, [0; 3])
                || !belt_connector::valid_axis(&g, end)
                || !belt_connector::can_connect(&g, [0; 3], end)
            {
                return Ok("0".into());
            }
            let chain = belt_connector::layout([0; 3], end, axis(n(v, 1) as usize), |p| {
                belt_connector::Ground::shaft(&g, p)
            });
            let segment = chain[0].1;
            Ok(format!(
                "1 {} {} {}",
                segment.slope as u8,
                segment.facing as u8,
                chain.len()
            ))
        }
        "M" => {
            let rpm = f(v, 1);
            let facing = Dir::ALL[n(v, 2) as usize];
            Ok(format!(
                "{} {} {}",
                belt::movement_speed(rpm).to_bits() as i32,
                belt::direction_aware_speed(rpm, facing).to_bits() as i32,
                belt::movement_facing(rpm, facing) as u8
            ))
        }
        "E" => Err("end-ejection velocity is private SDK glue, not the public inventory operation"),
        "G" => Err("getStackAtOffset has no public read-only engine equivalent"),
        "I" => {
            let mut item = belt::Item::new("parity:item", 1, 180, false);
            item.position = f(v, 6);
            item.inserted_at = n(v, 7);
            item.inserted_from = Some(Dir::ALL[n(v, 8) as usize]);
            let inv = belt::Inventory::from_saved(vec![item], n(v, 3) != 0);
            Ok(u8::from(inv.can_insert_at_from_side(
                n(v, 4),
                Dir::ALL[n(v, 5) as usize],
                belt::movement_facing(f(v, 1), Dir::ALL[n(v, 2) as usize]),
            ))
            .to_string())
        }
        "T" => {
            if n(v, 6) == 2 {
                return Err(
                    "synthetic FUNNEL ending has no engine enum value; native funnels require their own surroundings fixture",
                );
            }
            let mut items = Vec::new();
            for key in 0..n(v, 11) as usize {
                let at = 12 + key * 6;
                let mut item =
                    belt::Item::new(&key.to_string(), n(v, at) as u8, n(v, at + 2), false);
                item.position = f(v, at + 1);
                item.side_offset = f(v, at + 3);
                item.locked = n(v, at + 4) != 0;
                item.held_externally = n(v, at + 5) != 0;
                items.push(item);
            }
            let mut inv = belt::Inventory::from_saved(items, n(v, 5) != 0);
            let mut effects = Effects {
                ending: match n(v, 6) {
                    0 => belt::Ending::Eject,
                    1 => belt::Ending::Insert,
                    _ => belt::Ending::Blocked,
                },
                room: n(v, 8) as u8,
                segment: n(v, 9),
                mode: n(v, 10),
            };
            let mut ejected = Vec::new();
            for _ in 0..n(v, 7) {
                ejected.extend(
                    inv.tick(
                        n(v, 4),
                        belt::direction_aware_speed(f(v, 1), Dir::ALL[n(v, 2) as usize]),
                        n(v, 3) == 0,
                        &mut effects,
                    )
                    .ejected,
                );
            }
            let mut out = format!("{} {} {}", effects.room, inv.items.len(), ejected.len());
            for i in inv.items {
                out.push_str(&format!(
                    " {}:{}:{}:{}:{}:{}",
                    i.item,
                    i.count,
                    i.position.to_bits() as i32,
                    i.side_offset.to_bits() as i32,
                    u8::from(i.locked),
                    u8::from(i.held_externally)
                ));
            }
            for i in ejected {
                out.push_str(&format!(" E{}:{}", i.item, i.count));
            }
            Ok(out)
        }
        _ => panic!("unknown belt operation"),
    }
}

pub fn processing(v: &[&str]) -> Answer {
    let mut items = Vec::new();
    for i in 0..n(v, 5) as usize {
        let at = 6 + i * 5;
        // A fixture record already queued for deletion never reaches the callback.
        if n(v, at + 4) != 0 {
            continue;
        }
        let mut item = belt::Item::new(v[at], n(v, at + 1) as u8, n(v, at + 3), false);
        item.position = f(v, at + 2);
        item.fan.time = 17;
        items.push(item);
    }
    let mut inv = belt::Inventory::from_saved(items, n(v, 0) != 0);
    let (mut calls, mut updates) = (0, 0);
    for _ in 0..n(v, 4) {
        updates += u32::from(inv.apply_within(f(v, 1), f(v, 2), |item| {
            calls += 1;
            item.fan.time -= 1;
            // The public Process contract already classifies no-op/equal outputs.
            // Fixture modes 0/1/3 express that contract; no production predicate is copied.
            if [0, 1, 3].contains(&n(v, 3)) {
                return belt::Process::Pass;
            }
            if n(v, 3) == 2 {
                return belt::Process::Convert {
                    held: None,
                    outputs: vec![],
                };
            }
            let mut copy = item.clone();
            copy.angle = 271;
            copy.position += 1.0;
            copy.item = (copy.item.parse::<i32>().expect("fixture item key") + 100).to_string();
            if n(v, 3) == 4 {
                return belt::Process::Convert {
                    held: None,
                    outputs: vec![copy],
                };
            }
            copy.count = 1;
            let mut left = item.clone();
            left.count -= 1;
            belt::Process::Convert {
                held: Some(left),
                outputs: if n(v, 3) == 5 { vec![copy] } else { vec![] },
            }
        }));
    }
    let mut records: Vec<_> = inv
        .to_save()
        .items
        .iter()
        .map(|i| {
            format!(
                "{}:{}:{}:{}:{}",
                i.item,
                i.count,
                i.position.to_bits() as i32,
                i.angle,
                i.fan.time
            )
        })
        .collect();
    records.sort();
    Ok(format!("{calls} {updates} {}", records.join(" ")))
}

pub fn surface(v: &[&str]) -> Answer {
    let seg = belt::Segment {
        facing: Dir::ALL[n(v, 0) as usize + 2],
        slope: [
            belt::Slope::Horizontal,
            belt::Slope::Upward,
            belt::Slope::Downward,
            belt::Slope::Vertical,
            belt::Slope::Sideways,
        ][n(v, 1) as usize],
        part: [
            belt::Part::Start,
            belt::Part::Middle,
            belt::Part::End,
            belt::Part::Pulley,
        ][n(v, 2) as usize],
    };
    // The Java fixture's client render clock is a float, independently of server ticks.
    let time = f(v, 4) + f(v, 5);
    let mut records = Vec::new();
    // Java's renderer visits bottom before top, skipping a second diagonal surface.
    let bottoms: &[bool] = if [belt::Slope::Upward, belt::Slope::Downward].contains(&seg.slope) {
        &[false]
    } else {
        &[true, false]
    };
    for &bottom in bottoms {
        let name = belt::partial(&seg, bottom).expect("fixture surface");
        let scroll = belt::scroll(&seg, f(v, 3), false, f64::from(time), bottom);
        records.push(format!(
            "{} {} {}",
            name.strip_prefix("create:part_belt_")
                .expect("model namespace"),
            u8::from(scroll.is_some()),
            scroll.unwrap_or(0.0)
        ));
    }
    Ok(records.join(" "))
}
