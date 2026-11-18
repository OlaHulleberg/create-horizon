//! Basin apply's public inventory contract. Routing/null capabilities require game glue.
use super::{Answer, f, n};
use create_engine::{basin, fan::Heat, processing, tank::Fluid};

fn fluid(key: i32) -> Fluid {
    [Fluid::Water, Fluid::Lava, Fluid::Honey][key as usize]
}
fn fluid_id(f: Fluid) -> i32 {
    match f {
        Fluid::Water => 0,
        Fluid::Lava => 1,
        Fluid::Honey => 2,
        _ => unreachable!("fixture fluid"),
    }
}

pub fn probe(v: &[&str]) -> Answer {
    if n(v, 2) == 0 {
        return Err("ordinary crafting recipe coercion requires the game's recipe adapter");
    }
    if n(v, 5) != 0 {
        return Err("horizontal Basin output routing requires game capabilities");
    }
    if [8, 9, 10].iter().any(|&i| n(v, i) == 0) {
        return Err("null capabilities or a replaced Basin block require host validation");
    }
    let mut at = 11;
    let count = n(v, at) as usize;
    at += 1;
    let ingredients = (0..count)
        .map(|_| {
            let mask = n(v, at);
            at += 1;
            (0..32)
                .filter(|i| mask & (1 << i) != 0)
                .map(|i| i.to_string())
                .collect()
        })
        .collect();
    let count = n(v, at) as usize;
    at += 1;
    let fluids = (0..count)
        .map(|_| {
            let entry = (fluid(n(v, at)), n(v, at + 1) as u32);
            at += 2;
            entry
        })
        .collect();
    let count = n(v, at) as usize;
    at += 1;
    let mut results = Vec::new();
    for _ in 0..count {
        if f(v, at + 3) != 1.0 {
            return Err(
                "Basin recipe uses scripted RNG, which the public concrete PRNG cannot accept",
            );
        }
        if n(v, at + 2) != 64 {
            return Err("per-stack limits differ from this fixture's fixed item limit");
        }
        results.push(processing::Output {
            item: n(v, at).to_string(),
            count: n(v, at + 1) as u8,
            chance: 1.0,
        });
        at += 4;
    }
    let count = n(v, at) as usize;
    at += 1;
    let fluid_results = (0..count)
        .map(|_| {
            let entry = (fluid(n(v, at)), n(v, at + 1) as u32);
            at += 2;
            entry
        })
        .collect();
    let mut slots = Vec::new();
    for _ in 0..18 {
        if n(v, at + 2) != 64 {
            return Err("per-stack limits differ from this fixture's fixed item limit");
        }
        slots.push((n(v, at + 1) > 0).then(|| (n(v, at).to_string(), n(v, at + 1) as u8)));
        at += 3;
    }
    let mut tanks = [None; 4];
    for tank in &mut tanks {
        *tank = (n(v, at + 1) > 0).then(|| (fluid(n(v, at)), n(v, at + 1) as u32));
        at += 2;
    }
    assert_eq!(at, v.len(), "Basin fixture columns");
    // The authored Java context enumerates output tanks before input tanks.
    let mut contents = basin::Tanks {
        output: [tanks[0], tanks[1]],
        input: [tanks[2], tanks[3]],
    };
    let recipe = basin::Recipe {
        id: "parity:basin".into(),
        ingredients,
        results,
        time: 0,
        heat: [
            basin::HeatReq::None,
            basin::HeatReq::Heated,
            basin::HeatReq::Superheated,
        ][n(v, 4) as usize],
        fluids,
        fluid_results,
    };
    let heat = [
        Heat::None,
        Heat::Smouldering,
        Heat::Fading,
        Heat::Kindled,
        Heat::Seething,
    ][n(v, 3) as usize];
    let success = basin::apply(
        &mut slots,
        &mut contents,
        &recipe,
        heat,
        |_| 64,
        &mut processing::Rng::new(1),
        n(v, 1) != 0,
    );
    let mut out = u8::from(success).to_string();
    for slot in slots {
        out.push_str(&match slot {
            Some((item, count)) => format!(" {item}:{count}:64"),
            None => " -1:0:64".into(),
        });
    }
    for tank in contents.output.into_iter().chain(contents.input) {
        out.push_str(&match tank {
            Some((f, amount)) => format!(" {}:{amount}", fluid_id(f)),
            None => " -1:0".into(),
        });
    }
    out.push_str(" I F");
    Ok(out)
}
