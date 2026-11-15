//! The frogport drawn (`FrogportRenderer`): its body turned toward its target (or as placed),
//! its head opening and its tongue reaching to the target while a package is caught or sent,
//! the package riding the tongue. What clients need is the port's state in its block entity's
//! data (`Frog`), written when it changes: the target's exact place, the placed yaw, and the
//! tick an animation started, whether it deposits, and its package.
//!
//! Left out: the anticipation before a package arrives (`anticipate`), the head opening while
//! its screen is open (`manualOpenAnimationProgress`), goggles on a frogport, and the address
//! shown over it on hover.

use super::super::server::ServerLevel;
use super::{body, chain, with};
use mod_api::{
    BlockEntityDrawContext, BlockEntityPart, BlockEntitySubmitEvent, BlockPart, ClientHost,
    Compound, ItemDisplayContext, Tag, TerrainModelOptions,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The key of a frogport's state in its block entity's data.
pub const FROG: &str = "Frog";

const MODELS: [&str; 3] = [
    "create:block/package_frogport/body",
    "create:block/package_frogport/head",
    "create:block/package_frogport/tongue",
];

pub(super) fn models() -> impl Iterator<Item = String> {
    MODELS.iter().map(|m| (*m).to_owned())
}

/// What clients are told of a frogport.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    target: Option<[f64; 3]>,
    yaw: f32,
    animating: bool,
    deposit: bool,
    item: String,
}

thread_local! {
    static SENT: RefCell<HashMap<(usize, [i32; 3]), Shown>> = RefCell::new(HashMap::new());
    static MODEL_REFS: RefCell<HashMap<&'static str, mod_api::ModelRef>> = RefCell::new(HashMap::new());
}

pub(super) fn reset() {
    SENT.with_borrow_mut(HashMap::clear);
}

/// Each frogport's state to its block entity's data, when it changed; an animation starting
/// brings the tick it started.
pub(super) fn sync(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now = lv.game_time();
    let host: &dyn mod_api::Host = &*lv.host;
    let shown: Vec<([i32; 3], Shown)> = with(dim, |pk| {
        pk.ports
            .iter()
            .filter(|(_, p)| p.frog)
            .map(|(pos, p)| {
                let target = p.target.and_then(|t| {
                    let c = pk.chains.conveyors.get(&t.conveyor)?;
                    Some(chain::package_position(
                        t.conveyor,
                        c,
                        t.chain_pos,
                        t.connection,
                    ))
                });
                (
                    *pos,
                    Shown {
                        target,
                        yaw: p.yaw,
                        animating: p.is_animating(),
                        deposit: p.deposit,
                        item: p
                            .animated
                            .as_ref()
                            .map(|b| b.stack.item().id(host))
                            .unwrap_or_default(),
                    },
                )
            })
            .collect()
    });
    for (pos, s) in shown {
        let before = SENT.with_borrow(|m| m.get(&(dim, pos)).cloned());
        if before.as_ref() == Some(&s) {
            continue;
        }
        let started = s.animating && !before.as_ref().is_some_and(|b| b.animating);
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let mut c = vec![
            ("Yaw", Tag::Float(s.yaw.to_bits())),
            ("Animating", Tag::Byte(i8::from(s.animating))),
            ("Deposit", Tag::Byte(i8::from(s.deposit))),
            ("Item", Tag::String(s.item.as_str().into())),
        ];
        if let Some(t) = s.target {
            c.push((
                "Target",
                Tag::List(t.iter().map(|v| Tag::Double(v.to_bits())).collect()),
            ));
        }
        if started {
            c.push(("Start", Tag::Long(now)));
        } else if let Some(Tag::Compound(old)) = entity
            .data(&*lv.host)
            .and_then(|d| d.get(&FROG.into()).cloned())
            && let Some(start) = old.get(&"Start".into())
        {
            c.push(("Start", start.clone()));
        }
        let data = Compound::from([(FROG.into(), Tag::compound(c))]);
        if entity.merge_data(&mut *lv.host, data).is_ok() {
            SENT.with_borrow_mut(|m| m.insert((dim, pos), s));
        }
    }
}

fn model(draw: &mut dyn BlockEntityDrawContext, host: &dyn ClientHost, file: &'static str) {
    for fresh in [false, true] {
        let cached = (!fresh)
            .then(|| MODEL_REFS.with_borrow(|m| m.get(file).copied()))
            .flatten();
        let m = cached.or_else(|| {
            let id = mod_api::Identifier::parse(file).ok()?;
            let m = host.render_assets().model(&id).ok()?;
            MODEL_REFS.with_borrow_mut(|c| c.insert(file, m));
            Some(m)
        });
        let Some(m) = m else { return };
        match draw.terrain_model(m, TerrainModelOptions::default()) {
            Ok(()) => return,
            Err(mod_api::TerrainModelError::Model(_)) if !fresh => continue,
            Err(_) => return,
        }
    }
}

fn get_f(c: &Compound, k: &str) -> f32 {
    match c.get(&k.into()) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    }
}

fn flag(c: &Compound, k: &str) -> bool {
    matches!(c.get(&k.into()), Some(Tag::Byte(1)))
}

/// The frogport's drawing on the shared block entity type.
pub(super) struct FrogDraw;

#[mod_api::horizon_part]
impl BlockPart for FrogDraw {}

#[mod_api::horizon_part]
impl BlockEntityPart for FrogDraw {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        if event.block != "create:package_frogport" {
            return;
        }
        let pos = event.pos;
        let frog = match event.entity.nbt.get(&FROG.into()) {
            Some(Tag::Compound(c)) => c.clone(),
            _ => Compound::new(),
        };
        let draw = &mut *event.draw;
        let target = match frog.get(&"Target".into()) {
            Some(Tag::List(l)) if l.len() == 3 => {
                let v: Vec<f64> = l
                    .iter()
                    .map(|t| match t {
                        Tag::Double(d) => f64::from_bits(*d),
                        _ => 0.0,
                    })
                    .collect();
                Some([v[0], v[1], v[2]])
            }
            _ => None,
        };
        let animating = flag(&frog, "Animating");
        let deposit = flag(&frog, "Deposit");
        let center = pos.map(|v| f64::from(v) + 0.5);
        // `getYaw`.
        let yaw = match target {
            Some(t) => ((t[0] - center[0]).atan2(t[2] - center[2]).to_degrees() + 180.0) as f32,
            None => get_f(&frog, "Yaw"),
        };
        let mut head_pitch = 80.0f32;
        let mut tongue_pitch = 0.0f32;
        let mut tongue_length = 0.0f32;
        let mut diff = [0.0f64; 3];
        if let Some(t) = target {
            let drop = if animating && deposit { 0.0 } else { 0.75 };
            diff = [t[0] - center[0], t[1] - drop - center[1], t[2] - center[2]];
            let flat = (diff[0] * diff[0] + diff[2] * diff[2]).sqrt();
            tongue_pitch = diff[1].atan2(flat + 3.0 / 16.0).to_degrees() as f32;
            tongue_length = ((diff[0] * diff[0] + diff[1] * diff[1] + diff[2] * diff[2]).sqrt()
                as f32)
                .max(1.0);
            head_pitch = (tongue_pitch * 2.0).clamp(60.0, 100.0);
        }
        let head_modifier;
        let mut package: Option<(f32, f32)> = None;
        let mut moving = false;
        if animating {
            // `animationProgress`: linear, a tenth a tick from when it started.
            let start = match frog.get(&"Start".into()) {
                Some(Tag::Long(v)) => *v,
                _ => draw.game_time(),
            };
            let progress = ((draw.game_time() - start) as f32 + draw.partial()) / 10.0;
            let progress = progress.clamp(0.0, 1.0);
            moving = progress < 1.0;
            let (scale, distance);
            if deposit {
                let m = (1.0 - ((progress - 0.25) * 4.0 - 1.0).powi(4)).max(0.0);
                distance =
                    (tongue_length * (1.0f32).min((progress - 0.25) * 3.0)).max(tongue_length * m);
                tongue_length *= (1.0 - ((progress * 1.25 - 0.25) * 4.0 - 1.0).powi(4)).max(0.0);
                head_modifier = (1.0 - ((progress * 1.25) * 2.0 - 1.0).powi(4)).max(0.0);
                scale = 0.25 + progress * 3.0 / 4.0;
            } else {
                tongue_length *= (1.0 - progress * 1.25).max(0.0).powi(5);
                head_modifier = 1.0 - (((progress * 1.5).powi(2) - 0.5) * 2.0).clamp(0.0, 1.0);
                scale = (1.0 - progress * 1.25).max(0.5);
                distance = tongue_length;
            }
            package = Some((scale, distance));
        } else {
            tongue_length = 0.0;
            head_modifier = 0.0;
        }
        head_pitch *= head_modifier;
        // Body, head and tongue.
        draw.push();
        draw.rotate_about([0.0, 1.0, 0.0], yaw, [0.5; 3]);
        model(draw, host, MODELS[0]);
        draw.push();
        draw.rotate_about([1.0, 0.0, 0.0], head_pitch, [0.5, 10.0 / 16.0, 11.0 / 16.0]);
        model(draw, host, MODELS[1]);
        draw.pop();
        if tongue_length > 0.0 {
            draw.push();
            let pivot = [0.5, 10.0 / 16.0, 11.0 / 16.0];
            draw.translate(pivot);
            draw.rotate_about([1.0, 0.0, 0.0], tongue_pitch, [0.0; 3]);
            draw.scale([1.0, 1.0, tongue_length / (7.0 / 16.0)]);
            draw.translate(pivot.map(|v: f32| -v));
            model(draw, host, MODELS[2]);
            draw.pop();
        }
        draw.pop();
        // `renderPackage`.
        if let Some((scale, distance)) = package
            && scale >= 0.45
            && let Some(Tag::String(id)) = frog.get(&"Item".into())
            && let Some(stack) = body::new_stack(host, &id.to_string_lossy(), 1)
        {
            let l = (diff[0] * diff[0] + diff[1] * diff[1] + diff[2] * diff[2]).sqrt();
            let n = if l > 1e-9 {
                diff.map(|v| v / l)
            } else {
                [0.0; 3]
            };
            let drop = if deposit { 0.75 } else { 0.0 };
            draw.push();
            draw.translate([
                (n[0] * f64::from(distance)) as f32 + 0.5,
                3.0 / 16.0 + (n[1] * f64::from(distance) - drop) as f32 + 0.5,
                (n[2] * f64::from(distance)) as f32 + 0.5,
            ]);
            draw.scale([scale; 3]);
            draw.item(&stack, ItemDisplayContext::None, None, 0);
            draw.pop();
        }
        if !moving {
            draw.keep();
        }
    }
}
