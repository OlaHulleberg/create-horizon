//! Test-only adapters. Call the port's public functions; never translate the Java under test.
use create_engine::{
    Block, basin, burner, client, drill, fan, gauge, kinetics, place, press, processing, waterwheel,
};
use kinetics::{Axis, Dir, Part};
use std::cell::RefCell;
use std::io::{self, BufRead, Write};
mod basin_apply;
mod transport;

type Answer = Result<String, &'static str>;
fn axis(i: usize) -> Axis {
    [Axis::X, Axis::Y, Axis::Z][i]
}
fn f(v: &[&str], i: usize) -> f32 {
    v[i].parse().expect("fixture float")
}
fn n(v: &[&str], i: usize) -> i32 {
    v[i].parse().expect("fixture integer")
}
fn dir_name(d: Dir) -> &'static str {
    match d {
        Dir::Down => "down",
        Dir::Up => "up",
        Dir::North => "north",
        Dir::South => "south",
        Dir::West => "west",
        Dir::East => "east",
    }
}
fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}
fn placement(v: &[&str]) -> Answer {
    let look = Dir::ALL[n(v, 2) as usize];
    // The old fixture specifies an exact nearest direction, not a continuous yaw.
    let (yaw, pitch) = match look {
        Dir::Down => (0.0, 90.0),
        Dir::Up => (0.0, -90.0),
        Dir::North => (180.0, 0.0),
        Dir::South => (0.0, 0.0),
        Dir::West => (90.0, 0.0),
        Dir::East => (-90.0, 0.0),
    };
    let context = place::Context {
        yaw,
        pitch,
        sneaking: n(v, 4) != 0,
        clicked_face: Dir::ALL[n(v, 3) as usize],
    };
    let shaft = |d: Dir| n(v, 1) & i32::from(d.bit()) != 0;
    if n(v, 0) == 1 {
        return Ok(dir_name(place::facing(Block::CreativeMotor, context, shaft).unwrap()).into());
    }
    let nearby = place::Nearby {
        small_cog_clicked: match n(v, 5) {
            0 => None,
            k => Some(axis((k - 1) as usize)),
        },
        ..Default::default()
    };
    let block = if n(v, 0) == 0 {
        Block::Shaft
    } else {
        Block::Cogwheel
    };
    Ok(axis_name(place::axis(block, context, shaft, nearby).unwrap().0).into())
}
fn rotation(v: &[&str]) -> Answer {
    let node = |at: usize| {
        let a = axis(n(v, at + 1) as usize);
        match n(v, at) {
            0 | 4 => Part::shaft(a),
            1 | 2 => Part::cog(a, n(v, at) == 2),
            // The fixture supplies the rotation axis independently of a motor's facing.
            3 => Part {
                axis: a,
                ..Part::half(Dir::ALL[n(v, at + 2) as usize])
            },
            5 => Part::gearbox(a),
            6 => Part::split_shaft(a, if n(v, at + 3) == 0 { 1.0 } else { -1.0 }),
            7 => Part::split_shaft(a, if n(v, at + 3) == 0 { 1.0 } else { 0.0 }),
            _ => unreachable!("fixture kind"),
        }
    };
    let a = node(0);
    let b = node(4);
    let diff = [n(v, 8), n(v, 9), n(v, 10)];
    let mut ratio = kinetics::ratio(&a, &b, diff);
    // RotationPropagator's axis modifiers are separate public engine operations.
    // Supply the same explicit source faces as RotationOracle's fixture entities.
    if let Some(d) = Dir::from_offset(diff)
        && a.shafts & d.bit() != 0
        && b.shafts & d.opposite().bit() != 0
    {
        let source = |p: &Part, kind: i32| {
            if kind == 5 {
                Dir::from_axis(
                    [Axis::X, Axis::Y, Axis::Z]
                        .into_iter()
                        .find(|x| *x != p.axis)
                        .unwrap(),
                    true,
                )
            } else {
                Dir::from_axis(p.axis, true)
            }
        };
        ratio *= a.rotation.modifier(Some(source(&a, n(v, 0))), d);
        ratio /= b.rotation.modifier(Some(source(&b, n(v, 4))), d.opposite());
    }
    Ok(ratio.to_string())
}
fn fan_kind(i: i32) -> Option<fan::Kind> {
    match i {
        0 => Some(fan::Kind::Blasting),
        1 => Some(fan::Kind::Haunting),
        2 => Some(fan::Kind::Smoking),
        3 => Some(fan::Kind::Splashing),
        _ => None,
    }
}
fn fan_index(k: Option<fan::Kind>) -> i32 {
    match k {
        Some(fan::Kind::Blasting) => 0,
        Some(fan::Kind::Haunting) => 1,
        Some(fan::Kind::Smoking) => 2,
        Some(fan::Kind::Splashing) => 3,
        None => -1,
    }
}
fn fan_probe(v: &[&str]) -> Answer {
    match v[0] {
        "M" => Ok(fan::max_distance(f(v, 1)).to_string()),
        "V" => Ok(fan::propeller_rpm(f(v, 1)).to_string()),
        "P" => Ok(u8::from(fan::pushing(f(v, 1), Dir::ALL[n(v, 2) as usize])).to_string()),
        "D" | "T" => {
            let mut p = fan::Progress {
                by: fan_kind(n(v, 1)),
                time: n(v, 2),
            };
            let mut out = String::new();
            for phase in &v[3..] {
                let c: Vec<_> = phase.split(',').collect();
                let kind = fan_kind(n(&c, 0)).unwrap();
                let count = n(&c, 1) as u32;
                let can = n(&c, 2) != 0;
                for _ in 0..n(&c, 3) {
                    let converted = if v[0] == "T" {
                        fan::advance(&mut p, kind, count, can)
                    } else if fan::entity_can(&p, kind, can) {
                        fan::advance_entity(&mut p, kind, count)
                    } else {
                        false
                    };
                    out.push_str(&format!(
                        "{}:{}:{} ",
                        fan_index(p.by),
                        p.time,
                        u8::from(converted)
                    ));
                }
            }
            Ok(out)
        }
        _ => panic!("unknown fan operation"),
    }
}
fn fan_media(v: &[&str]) -> Answer {
    let fluid = n(v, 0);
    let block = n(v, 1);
    if fluid & 6 != 0 {
        return Err(
            "Catalyst has no inputs for haunting/smoking fluid tags; these Java contexts cannot be represented by the public engine model",
        );
    }
    let catalyst = fan::Catalyst {
        water: fluid & 8 != 0,
        lava: fluid & 1 != 0,
        splashing: block & 8 != 0,
        haunting: block & 2 != 0,
        smoking: block & 4 != 0,
        blasting: block & 1 != 0,
        campfire_lit: if n(v, 2) == 0 || n(v, 3) == 2 {
            None
        } else {
            Some(n(v, 3) != 0)
        },
        soul_flame: if n(v, 4) == 2 {
            None
        } else {
            Some(n(v, 4) != 0)
        },
        heat: [
            Some(fan::Heat::None),
            Some(fan::Heat::Smouldering),
            Some(fan::Heat::Fading),
            Some(fan::Heat::Kindled),
            Some(fan::Heat::Seething),
            None,
        ][n(v, 5) as usize],
    };
    // Ask the engine about each catalyst independently before asking its priority.
    // This supplies one candidate at a time, rather than reproducing the validity predicates.
    let mut mask = 0;
    for i in 0..4 {
        let only = fan::Catalyst {
            water: i == 3 && catalyst.water,
            lava: i == 0 && catalyst.lava,
            splashing: i == 3 && catalyst.splashing,
            haunting: i == 1 && catalyst.haunting,
            smoking: i == 2 && catalyst.smoking,
            blasting: i == 0 && catalyst.blasting,
            ..catalyst
        };
        if fan_index(fan::kind_at(&only)) == i {
            mask |= 1 << i;
        }
    }
    Ok(format!("{mask} {}", fan_index(fan::kind_at(&catalyst))))
}
fn fan_entity(v: &[&str]) -> Answer {
    if v[0] == "E" {
        return Err(
            "entity fire/status/horse-conversion effects are implemented in SDK glue, not the public engine",
        );
    }
    if n(v, 8) != 0 || n(v, 9) == 0 || (n(v, 6) != 0 && n(v, 7) != 0) {
        return Err(
            "entity eligibility requires the native entity adapter; only eligible motion is in this public operation",
        );
    }
    let coordinate = |i: usize| v[i].parse::<f64>().expect("fixture double");
    let distance = (10..13).map(|i| coordinate(i).powi(2)).sum::<f64>().sqrt();
    let direction = Dir::from_axis(axis(n(v, 3) as usize), n(v, 4) > 0);
    let motion = fan::push(
        direction,
        f(v, 1),
        f(v, 2),
        distance,
        [coordinate(13), coordinate(14), coordinate(15)],
        n(v, 5) != 0,
    );
    Ok(format!("{} {} {}", motion[0], motion[1], motion[2]))
}
fn milling(v: &[&str]) -> Answer {
    match v[0] {
        "S" => Ok(processing::processing_speed(f(v, 1)).to_string()),
        "R" => {
            let recipe = processing::Recipe {
                id: "parity:roll".into(),
                ingredients: vec![],
                time: 0,
                results: vec![processing::Output {
                    item: "parity:output".into(),
                    count: n(v, 2) as u8,
                    chance: f(v, 3),
                }],
            };
            let out = processing::roll(
                &recipe,
                &mut processing::Rng::new(v[1].parse().expect("fixture seed")),
            );
            Ok(out
                .iter()
                .map(|(_, n)| u32::from(*n))
                .sum::<u32>()
                .to_string())
        }
        "T" => {
            let recipe = processing::Recipe {
                id: "parity:milling".into(),
                ingredients: vec![vec!["parity:input".into()]],
                results: vec![processing::Output {
                    item: "parity:output".into(),
                    count: 1,
                    chance: 1.0,
                }],
                time: n(v, 5),
            };
            let book = processing::Book::new(if n(v, 4) != 0 { vec![recipe] } else { vec![] });
            let mut cycle = processing::Millstone {
                timer: n(v, 2),
                recipe: None,
            };
            let mut input = n(v, 3);
            let mut slots = [n(v, 6), 0, 0, 0, 0, 0, 0, 0, 0];
            let mut made = 0;
            for tick in 0..n(v, 7) {
                if tick == n(v, 8) {
                    input = 0;
                }
                let found = processing::tick_millstone(
                    &mut cycle,
                    f(v, 1),
                    if input > 0 {
                        Some("parity:input")
                    } else {
                        None
                    },
                    slots.iter().any(|c| *c >= 99),
                    &book,
                );
                if found.is_some() && input > 0 {
                    input -= 1;
                    made += 1;
                    if let Some(slot) = slots.iter_mut().find(|c| **c < 64) {
                        *slot += 1;
                    }
                }
            }
            Ok(format!(
                "{} {input} {} {made}",
                cycle.timer,
                slots.iter().sum::<i32>()
            ))
        }
        _ => panic!("unknown milling operation"),
    }
}
fn drilling(v: &[&str]) -> Answer {
    match v[0] {
        "S" => Err(
            "the public engine exposes Breaker::tick, not its standalone break-speed expression",
        ),
        "D" => Ok(drill::damage(f(v, 1)).to_string()),
        "T" => {
            let mut cycle = drill::Breaker {
                progress: n(v, 4),
                ticks_until: n(v, 5),
            };
            let (mut lazy, mut broken, mut hits) = (10, 0, 0);
            let hardness = if n(v, 3) == 0 {
                drill::breakable(f(v, 2))
            } else {
                None
            };
            for tick in 0..n(v, 6) {
                if tick == 0 || lazy == 0 {
                    cycle.lazy();
                }
                if lazy == 0 {
                    lazy = 10;
                } else {
                    lazy -= 1;
                }
                if tick == n(v, 7) {
                    cycle.ticks_until = 1;
                }
                let speed = if tick >= n(v, 8) && tick < n(v, 9) {
                    0.0
                } else {
                    f(v, 1)
                };
                match cycle.tick(speed, hardness) {
                    drill::Step::Broken => {
                        broken += 1;
                        hits += 1;
                    }
                    drill::Step::Progress(_) => hits += 1,
                    _ => {}
                }
            }
            Ok(format!(
                "{} {} {broken} {hits}",
                cycle.progress, cycle.ticks_until
            ))
        }
        _ => panic!("unknown drilling operation"),
    }
}
fn visual(v: &[&str]) -> Answer {
    match v[0] {
        "O" => Ok(client::rotation_offset(
            if n(v, 5) != 0 {
                Block::LargeCogwheel
            } else {
                Block::Shaft
            },
            axis(n(v, 1) as usize),
            [n(v, 2), n(v, 3), n(v, 4)],
        )
        .to_string()),
        "S" => Ok(gauge::speed_dial(f(v, 1)).to_string()),
        "W" => Ok(waterwheel::generated_speed(n(v, 1), n(v, 2)).to_string()),
        "A" => Err(
            "final time-to-angle evaluation belongs to the SDK draw path, outside create-engine",
        ),
        "C" => Err("crank client interpolation is private game glue, outside the public engine"),
        _ => panic!("unknown visual operation"),
    }
}
fn basin_probe(v: &[&str]) -> Answer {
    if v[0] == "A" {
        return basin_apply::probe(v);
    }
    if n(v, 11) != 0 {
        return Err("basin-removal publication is game glue, outside the public Mixer API");
    }
    let mut m = basin::Mixer {
        running: n(v, 4) != 0,
        running_ticks: n(v, 5),
        processing_ticks: n(v, 6),
        recipe: Some(0),
    };
    if n(v, 10) != 0 {
        m.start(0);
    }
    let (mut applied, mut scheduled) = (0, 0);
    for t in 0..n(v, 7) {
        let rpm = if n(v, 8) >= 0 && t >= n(v, 8) {
            0.0
        } else {
            f(v, 1)
        };
        match m.tick(rpm, n(v, 3)) {
            basin::MixStep::Apply => {
                applied += 1;
                // The Java fixture's authored apply callback can repeat the recipe.
                if n(v, 9) != 0 {
                    m.running_ticks = 20;
                }
            }
            basin::MixStep::Done => scheduled += 1,
            basin::MixStep::None => {}
        }
    }
    Ok(format!(
        "{} {} {} {applied} {scheduled}",
        u8::from(m.running),
        m.running_ticks,
        m.processing_ticks
    ))
}
fn burner_probe(v: &[&str]) -> Answer {
    if v[0] == "R" {
        return Err("burner datamap/tag resolution is game glue, not a public engine operation");
    }
    let fuels = [
        burner::Fuel::None,
        burner::Fuel::Normal,
        burner::Fuel::Special,
    ];
    let shown = [
        fan::Heat::None,
        fan::Heat::Smouldering,
        fan::Heat::Fading,
        fan::Heat::Kindled,
        fan::Heat::Seething,
    ][n(v, 4) as usize];
    let mut b = burner::Burner {
        fuel: fuels[n(v, 1) as usize],
        remaining: n(v, 2),
        creative: if n(v, 3) != 0 { Some(shown) } else { None },
    };
    let accepted = match v[0] {
        "T" => {
            for _ in 0..n(v, 5) {
                if b.burning() {
                    b.tick();
                }
            }
            false
        }
        "C" => {
            b.creative_fuel(shown);
            true
        }
        "I" => {
            let mut candidate = b;
            let accepted = candidate.try_fuel(fuels[n(v, 5) as usize], n(v, 6), n(v, 7) != 0);
            // SIMULATE is a host transaction: observe the real transition on a
            // private state copy, publishing it only for EXECUTE.
            if n(v, 8) == 0 {
                b = candidate;
            }
            accepted
        }
        _ => panic!("unknown burner operation"),
    };
    Ok(format!(
        "{} {} {} {}",
        u8::from(accepted),
        b.fuel.index(),
        b.remaining,
        u8::from(b.creative.is_some())
    ))
}
fn dispatch(suite: &str, v: &[&str]) -> Answer {
    match suite {
        "motion-primitives" => match v[0] {
            "W" => Ok(
                create_engine::contraption::windmill_speed(n(v, 1) as u32, n(v, 2) == 0)
                    .to_string(),
            ),
            "R" => Ok(create_engine::pulley::extension_range(n(v, 1), n(v, 2)).to_string()),
            "L" => Ok(create_engine::pulley::movement(f(v, 1)).to_string()),
            "G" => Ok(create_engine::gantry::pinion_speed(f(v, 1)).to_string()),
            "P" => Ok(create_engine::piston::movement(
                f(v, 1),
                Dir::ALL[n(v, 2) as usize],
                f(v, 3),
                n(v, 4),
            )
            .to_string()),
            _ => panic!("unknown motion primitive"),
        },
        "transport" => transport::probe(v),
        "transport-processing" => transport::processing(v),
        "belt-surface" => transport::surface(v),
        "network" => {
            let mut k = kinetics::Kinetics::new();
            k.set(
                [0, 0, 0],
                0,
                Part {
                    generated: f(v, 0),
                    capacity: f(v, 1),
                    ..Part::half(Dir::East)
                },
            );
            for x in 1..=n(v, 2) {
                k.set(
                    [x, 0, 0],
                    0,
                    Part {
                        impact: 8.0,
                        ..Part::shaft(Axis::X)
                    },
                );
            }
            k.solve(&mut Vec::new());
            let (_, stress, capacity, _) = k.network([0, 0, 0], 0).unwrap();
            Ok(format!("{{\"capacity\":{capacity},\"stress\":{stress}}}"))
        }
        "processing" => match v[0] {
            "S" => Ok(press::tick_speed(f(v, 1)).to_string()),
            "H" => Ok(press::head_offset(f(v, 1)).to_string()),
            "D" => Err(
                "bulk press routing needs the host's recipe/inventory callbacks; no isolated public operation",
            ),
            _ => panic!("unknown processing operation"),
        },
        "placement" => placement(v),
        "rotation" => rotation(v),
        "fan" => fan_probe(v),
        "fan-media" => fan_media(v),
        "fan-entity" => fan_entity(v),
        "basin" => basin_probe(v),
        "burner" => burner_probe(v),
        "milling" => milling(v),
        "drilling" => drilling(v),
        "visual" => visual(v),
        _ => panic!("unknown suite"),
    }
}
fn main() {
    let suite = std::env::args().nth(1).expect("suite argument");
    thread_local! { static PANIC_AT: RefCell<Option<String>> = const { RefCell::new(None) }; }
    std::panic::set_hook(Box::new(|info| {
        PANIC_AT.with(|at| {
            *at.borrow_mut() = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
        });
    }));
    let mut out = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let line = line.expect("fixture line");
        let words: Vec<_> = line.split_whitespace().collect();
        assert!(!words.is_empty(), "empty fixture");
        let result = std::panic::catch_unwind(|| dispatch(&suite, &words));
        match result {
            Ok(Ok(value)) => writeln!(out, "{value}"),
            Ok(Err(reason)) => writeln!(out, "!unsupported {reason}"),
            Err(panic) => {
                let at = PANIC_AT
                    .with(|at| at.borrow_mut().take())
                    .unwrap_or_default()
                    .replace('\\', "/");
                let engine = at.contains("/create/src/engine/src/");
                let at = at.split("/create/src/engine/src/").last().unwrap_or(&at);
                writeln!(
                    out,
                    "!{} {} ({at})",
                    if engine { "panic" } else { "harness" },
                    panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("non-string panic")
                        .replace(['\r', '\n'], " ")
                )
            }
        }
        .expect("probe output");
    }
}
