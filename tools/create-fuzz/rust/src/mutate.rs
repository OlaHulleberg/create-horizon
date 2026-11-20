//! Bounded mutations reuse legal states from the generator and preserve the settling window.
use crate::{
    generate::{self, Rng},
    scenario::Scenario,
};
pub fn mutate(parent: &Scenario, seed: u64) -> Scenario {
    let mut rng = Rng::new(seed);
    let mut sc = parent.clone();
    sc.seed = seed;
    if sc.version == 3 {
        for e in &mut sc.events {
            if let Some(crate::actions::Action::Item { input }) = &mut e.action {
                use crate::transport::Input;
                let stack = match input {
                    Input::DropOn { stack } => Some(stack),
                    Input::Use { stack, .. } => stack.as_mut(),
                };
                if let Some(s) = stack {
                    s.count = (1 + rng.below(u32::from(s.limit()))) as u8;
                }
            }
            if let Some(crate::actions::Action::ChainLink { connect, .. }) = &mut e.action {
                *connect = rng.chance(0.5);
            }
            if let Some(crate::actions::Action::ValveSetting { row, value }) = &mut e.action {
                *row = rng.below(2) as i32;
                *value = rng.below(181) as i32;
            }
            if let Some(crate::actions::Action::Program { instructions }) = &mut e.action {
                for i in instructions {
                    i.modifier = *rng.pick(&[
                        crate::program::Modifier::Forward,
                        crate::program::Modifier::ForwardFast,
                        crate::program::Modifier::Back,
                        crate::program::Modifier::BackFast,
                    ]);
                }
            }
            if let Some(crate::actions::Action::SpeedSetting { row, value }) = &mut e.action {
                *row = rng.below(2) as i32;
                *value = rng.below(257) as i32;
            }
            if let Some(crate::actions::Action::Signal { strength }) = &mut e.action {
                *strength = rng.below(16) as u8;
            }
            if let Some(crate::actions::Action::Use {
                yaw,
                sneaking,
                face,
                hit,
                ..
            }) = &mut e.action
            {
                *yaw = rng.below(360) as f32;
                *sneaking = rng.chance(0.5);
                let u = *rng.pick(&[0.1, 0.5, 0.9]);
                let v = *rng.pick(&[0.1, 0.5, 0.9]);
                let normal = face.dir().offset();
                let depth = *hit;
                *hit = face.point(u, v);
                for axis in 0..3 {
                    if normal[axis] != 0 {
                        hit[axis] = depth[axis];
                    }
                }
            }
        }
        return sc;
    }
    let donor = generate::generate(seed);
    let i = rng.below(sc.blocks.len() as u32) as usize;
    match rng.below(5) {
        0 if sc.blocks.len() > 1 => {
            let pos = sc.blocks.remove(i).pos;
            sc.events.retain(|e| e.pos != pos);
        }
        1 => {
            let mut b = rng.pick(&donor.blocks).clone();
            b.pos = sc.blocks[i].pos;
            sc.events.retain(|e| e.pos != b.pos || e.turn.is_none());
            sc.blocks[i] = b;
        }
        2 => {
            for b in donor.blocks {
                if !sc.blocks.iter().any(|old| old.pos == b.pos)
                    && !sc.events.iter().any(|e| e.pos == b.pos)
                {
                    sc.blocks.push(b);
                    break;
                }
            }
        }
        3 if !sc.events.is_empty() => {
            let j = rng.below(sc.events.len() as u32) as usize;
            sc.events[j].tick = sc.blocks.len() as u32 + rng.below(120);
        }
        _ => {
            if let Some(b) = sc.blocks.iter_mut().find(|b| b.motor.is_some()) {
                b.motor = Some(*rng.pick(&[-256, -128, -1, 1, 128, 256]));
            }
        }
    }
    sc.events.sort_by_key(|e| e.tick);
    let last = sc
        .events
        .iter()
        .map(|e| e.tick)
        .max()
        .unwrap_or(0)
        .max(sc.blocks.len() as u32);
    sc.ticks = (last + generate::SETTLE).max(sc.checkpoints.iter().copied().max().unwrap_or(0));
    sc
}
#[cfg(test)]
mod tests {
    #[test]
    fn use_mutations_preserve_partial_surface_depth() {
        let sc = crate::transmission::valve(10);
        let changed = super::mutate(&sc, 42);
        for (before, after) in sc.events.iter().zip(&changed.events) {
            if let (
                Some(crate::actions::Action::Use { face, hit: a, .. }),
                Some(crate::actions::Action::Use { hit: b, .. }),
            ) = (&before.action, &after.action)
            {
                for axis in 0..3 {
                    if face.dir().offset()[axis] != 0 {
                        assert_eq!(a[axis], b[axis]);
                    }
                }
            }
        }
    }

    #[test]
    fn mutations_stay_bounded_and_settle() {
        for seed in 0..1000 {
            let sc = super::mutate(&crate::generate::generate(seed / 5), seed);
            crate::catalogue::scenario(&sc);
            assert!(!sc.blocks.is_empty());
            assert!(sc.blocks.iter().all(|b| b.pos[0].abs() <= 23
                && b.pos[2].abs() <= 23
                && (1..=16).contains(&b.pos[1])));
            assert!(
                sc.events
                    .iter()
                    .all(|e| e.tick + crate::generate::SETTLE <= sc.ticks),
                "seed {seed}: an event is too close to the end",
            );
            assert!(sc.blocks.len() as u32 + crate::generate::SETTLE <= sc.ticks);
        }
    }
}
