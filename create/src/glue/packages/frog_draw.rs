//! The frogport drawn (`FrogportRenderer`): its body turned toward its target (or as placed),
//! its head opening and its tongue reaching to the target while a package is caught or sent,
//! the package riding the tongue. What clients need is the port's state in its block entity's
//! data (`Frog`), written when it changes: the target's exact place, the placed yaw, and the
//! tick an animation started, whether it deposits, and its package.
//!
//! A package for it a few ticks away opens its mouth (`anticipate`, `anticipationProgress`), and
//! so does its screen while a player has it open (`manualOpenAnimationProgress`), from the ticks
//! each began in the same data. Its sounds (`FrogportSounds`) and the rope's dust as it catches
//! are the server's, played where the clients would play them.

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

const MODELS: [&str; 4] = [
    "create:block/package_frogport/body",
    "create:block/package_frogport/head",
    "create:block/package_frogport/tongue",
    "create:block/package_frogport/head_goggles",
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
    goggles: bool,
    /// `addressFilter`, shown over it on hover.
    address: String,
    /// The tick its last anticipation began (`anticipationProgress` chasing 1 at 0.1 a tick).
    anticipated: Option<i64>,
    /// The ticks its screen was last opened and closed (`manualOpenAnimationProgress`).
    opened: Option<i64>,
    closed: Option<i64>,
}

/// What a frogport keeps for its clients beside the engine's state: its anticipations seen
/// (the engine's count) and when the last began, the players with its screen open, and when
/// it was last opened and closed.
#[derive(Default)]
struct Kept {
    anticipations: u32,
    anticipated: Option<i64>,
    viewers: Vec<u128>,
    /// Whether it had viewers at the last look.
    open: bool,
    opened: Option<i64>,
    closed: Option<i64>,
}

thread_local! {
    /// The block under the crosshair at the last client tick.
    static HOVERED: std::cell::Cell<Option<[i32; 3]>> = const { std::cell::Cell::new(None) };
    /// The client level's game time at the last client tick.
    static NOW: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };

    static SENT: RefCell<HashMap<(usize, [i32; 3]), Shown>> = RefCell::new(HashMap::new());
    static KEPT: RefCell<HashMap<(usize, [i32; 3]), Kept>> = RefCell::new(HashMap::new());
    static MODEL_REFS: RefCell<HashMap<&'static str, mod_api::ModelRef>> = RefCell::new(HashMap::new());
}

pub(super) fn reset() {
    SENT.with_borrow_mut(HashMap::clear);
    KEPT.with_borrow_mut(HashMap::clear);
}

/// A client tick: the block under the crosshair, for the nameplates.
pub(super) fn client_tick(host: &dyn ClientHost) {
    let hovered = match host.crosshair_target() {
        Some(mod_api::CrosshairTarget::Block { pos, .. }) => Some([pos.x, pos.y, pos.z]),
        _ => None,
    };
    HOVERED.set(hovered);
    if let Some(t) = host.level(host.dimension()).and_then(|l| l.game_time(host)) {
        NOW.set(t);
    }
}

/// Whether the frogport moves and is drawn again each frame (`Subsystem::animated`; a still
/// one's drawing is kept): while it catches or sends, anticipates, and opens or shuts with its
/// screen. One with an address always is: a kept drawing is not asked again when the crosshair
/// comes onto it, for its nameplate.
pub(super) fn animated(nbt: &Compound) -> bool {
    let Some(Tag::Compound(frog)) = nbt.get(&FROG.into()) else {
        return false;
    };
    let now = NOW.get();
    let long = |k: &str| match frog.get(&k.into()) {
        Some(Tag::Long(v)) => Some(*v),
        _ => None,
    };
    let within = |t: Option<i64>, ticks: i64| t.is_some_and(|t| (0..=ticks).contains(&(now - t)));
    let addressed = matches!(frog.get(&"Address".into()),
        Some(Tag::String(a)) if !a.to_string_lossy().trim().is_empty());
    flag(frog, "Animating") && within(long("Start"), 11)
        || within(long("Anticipated"), 11)
        || within(long("Opened"), 4)
        || within(long("Closed"), 4)
        || addressed
}

/// `openTracker`: `player` opened the screen of the port at `pos` (`open`) or closed it.
pub(super) fn viewer(dim: usize, pos: [i32; 3], player: u128, open: bool) {
    KEPT.with_borrow_mut(|k| {
        let kept = k.entry((dim, pos)).or_default();
        kept.viewers.retain(|p| *p != player);
        if open {
            kept.viewers.push(player);
        }
    });
}

/// `manualOpenAnimationProgress` at `now` (ticks with the partial tick): chasing 1 at 0.35 a
/// tick from when it was opened, then 0 from where it was when it closed.
fn manual_open(opened: Option<i64>, closed: Option<i64>, now: f32) -> f32 {
    let Some(opened) = opened else { return 0.0 };
    let rising = |at: f32| ((at - opened as f32) * 0.35).clamp(0.0, 1.0);
    match closed {
        Some(closed) if closed >= opened => {
            (rising(closed as f32) - (now - closed as f32) * 0.35).clamp(0.0, 1.0)
        }
        _ => rising(now),
    }
}

/// Each frogport's state to its block entity's data, when it changed; an animation starting
/// brings the tick it started.
pub(super) fn sync(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now = lv.game_time();
    let host: &dyn mod_api::Host = &*lv.host;
    let players: Vec<u128> = lv.players().iter().map(|p| p.uuid()).collect();
    let shown: Vec<([i32; 3], Shown)> = with(dim, |pk| {
        pk.ports
            .iter()
            .filter(|(_, p)| p.frog)
            .map(|(pos, p)| {
                let (anticipated, opened, closed) = KEPT.with_borrow_mut(|k| {
                    let kept = k.entry((dim, *pos)).or_default();
                    // Told again while its last is under way (`chase(1)` again), it goes on.
                    if kept.anticipations != p.anticipations {
                        kept.anticipations = p.anticipations;
                        if kept.anticipated.is_none_or(|a| now - a > 10) {
                            kept.anticipated = Some(now);
                        }
                    }
                    // A viewer gone from the level has closed it.
                    kept.viewers.retain(|v| players.contains(v));
                    let open = !kept.viewers.is_empty();
                    if open && !kept.open {
                        kept.opened = Some(now);
                    } else if kept.open && !open {
                        kept.closed = Some(now);
                    }
                    kept.open = open;
                    (kept.anticipated, kept.opened, kept.closed)
                });
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
                        goggles: p.goggles,
                        address: p.filter.clone(),
                        anticipated,
                        opened,
                        closed,
                    },
                )
            })
            .collect()
    });
    let gone: Vec<[i32; 3]> = KEPT.with_borrow(|k| {
        k.keys()
            .filter(|(d, _)| *d == dim)
            .map(|(_, p)| *p)
            .filter(|p| !shown.iter().any(|(s, _)| s == p))
            .collect()
    });
    KEPT.with_borrow_mut(|k| {
        for p in gone {
            k.remove(&(dim, p));
        }
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
        if started {
            sounds_on_start(lv, pos, &s);
        }
        if before.is_some()
            && s.closed.is_some()
            && before.as_ref().is_some_and(|b| b.closed != s.closed)
        {
            // `sounds.close` when the screen's animation has closed again: at once here.
            play(
                lv,
                pos,
                "create:frogport_close",
                1.0,
                1.25 + random(lv, pos) * 0.25,
            );
        }
        if before.is_some()
            && s.opened.is_some()
            && before.as_ref().is_some_and(|b| b.opened != s.opened)
        {
            // `onOpenedManually`.
            play(lv, pos, "create:frogport_open", 0.5, 1.0);
        }
        let mut c = vec![
            ("Yaw", Tag::Float(s.yaw.to_bits())),
            ("Animating", Tag::Byte(i8::from(s.animating))),
            ("Deposit", Tag::Byte(i8::from(s.deposit))),
            ("Item", Tag::String(s.item.as_str().into())),
            ("Goggles", Tag::Byte(i8::from(s.goggles))),
            ("Address", Tag::String(s.address.as_str().into())),
        ];
        for (k, v) in [
            ("Anticipated", s.anticipated),
            ("Opened", s.opened),
            ("Closed", s.closed),
        ] {
            if let Some(v) = v {
                c.push((k, Tag::Long(v)));
            }
        }
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

/// `FrogportSounds` and `startAnimation`'s client half: it opens, and deposits or catches,
/// the rope's dust falling where it caught the package.
fn sounds_on_start(lv: &mut ServerLevel<'_>, pos: [i32; 3], s: &Shown) {
    play(lv, pos, "create:frogport_open", 0.5, 1.0);
    if s.deposit {
        play(lv, pos, "create:frogport_deposit", 1.0, 1.0);
        return;
    }
    play(lv, pos, "create:frogport_catch", 1.0, 1.0);
    let (Some(t), Some(&key)) = (s.target, super::BLOCK_PARTICLE.get()) else {
        return;
    };
    let Some(rope) = lv.host.block_state_id("create:rope") else {
        return;
    };
    let options = mod_api::ParticleOptions::simple(key);
    // Five at the point, up to a quarter below it.
    let _ = lv.level.send_particles(
        &mut *lv.host,
        &options,
        Some(rope),
        [t[0], t[1] - 0.125, t[2]],
        [0.0, 0.0625, 0.0],
        0.0,
        5,
    );
}

fn play(lv: &mut ServerLevel<'_>, pos: [i32; 3], sound: &str, volume: f32, pitch: f32) {
    let at = pos.map(|v| f64::from(v) + 0.5);
    let _ = lv.level.play_sound(
        &mut *lv.host,
        sound,
        at,
        mod_api::SoundSource::Blocks,
        volume,
        pitch,
    );
}

/// A number in 0..1 for a sound's pitch, from the tick and place.
fn random(lv: &ServerLevel<'_>, pos: [i32; 3]) -> f32 {
    let mut x = (lv.game_time() as u64)
        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pos[1] as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (pos[2] as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// `FrogportBlockEntity.use`: goggles in the main hand go on a frogport without them (the
/// stack is kept), with the gold armour's sound.
pub(super) fn put_on_goggles(lv: &mut ServerLevel<'_>, click: &mod_api::BlockClick<'_>) -> bool {
    let host: &dyn mod_api::Host = &*lv.host;
    if click.hand != mod_api::Hand::Main
        || click
            .stack
            .is_none_or(|s| s.item().id(host) != "create:goggles")
    {
        return false;
    }
    let pos = click.pos;
    let done = with(lv.dim, |pk| match pk.ports.get_mut(&pos) {
        Some(p) if p.frog && !p.goggles => {
            p.goggles = true;
            pk.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            true
        }
        _ => false,
    });
    if done {
        let at = pos.map(|v| f64::from(v) + 0.5);
        let _ = lv.level.play_sound(
            &mut *lv.host,
            "minecraft:item.armor.equip_gold",
            at,
            mod_api::SoundSource::Blocks,
            0.5,
            1.0,
        );
    }
    done
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
            // `anticipationProgress`: 0 to 1 in ten ticks from when it was told, then 0 again.
            let now = (draw.game_time() as f32) + draw.partial();
            let a = match frog.get(&"Anticipated".into()) {
                Some(Tag::Long(t)) => {
                    let a = (now - *t as f32) / 10.0;
                    if (0.0..1.0).contains(&a) {
                        moving = true;
                        a
                    } else {
                        0.0
                    }
                }
                _ => 0.0,
            };
            head_modifier = if a > 0.0 {
                (1.0 - ((a * 1.25) * 2.0 - 1.0).powi(4)).max(0.0)
            } else {
                0.0
            };
        }
        head_pitch *= head_modifier;
        let long = |k: &str| match frog.get(&k.into()) {
            Some(Tag::Long(v)) => Some(*v),
            _ => None,
        };
        let now = (draw.game_time() as f32) + draw.partial();
        let manual = manual_open(long("Opened"), long("Closed"), now);
        if manual > 0.0 && manual < 1.0 {
            moving = true;
        }
        head_pitch = head_pitch.max(manual * 60.0);
        tongue_length = tongue_length.max(manual * 0.25);
        // Body, head and tongue.
        draw.push();
        draw.rotate_about([0.0, 1.0, 0.0], yaw, [0.5; 3]);
        model(draw, host, MODELS[0]);
        draw.push();
        draw.rotate_about([1.0, 0.0, 0.0], head_pitch, [0.5, 10.0 / 16.0, 11.0 / 16.0]);
        // `FROGPORT_HEAD_GOGGLES` on one wearing goggles.
        model(
            draw,
            host,
            MODELS[if flag(&frog, "Goggles") { 3 } else { 1 }],
        );
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
        let plated = nameplate(draw, host, pos, &frog);
        if !moving && !plated {
            draw.keep();
        }
    }
}

/// `renderNameplateOnHover` with the address (`FrogportRenderer`): while the crosshair is on
/// the frogport, within 64 blocks, its address a block and a quarter up, turned to the camera,
/// a fortieth of a block a font pixel. Whether the frogport has an address (it is drawn again
/// next frame, for the crosshair coming onto it or the camera turning). The name tag's see-through pass over its translucent
/// background is left out: a block entity's text is drawn only as a sign's (`API_GAPS.md`).
fn nameplate(
    draw: &mut dyn BlockEntityDrawContext,
    host: &dyn ClientHost,
    pos: [i32; 3],
    frog: &Compound,
) -> bool {
    let Some(Tag::String(address)) = frog.get(&"Address".into()) else {
        return false;
    };
    let address = address.to_string_lossy();
    if address.trim().is_empty() {
        return false;
    }
    nameplate_hovered(draw, host, pos, &address);
    true
}

fn nameplate_hovered(
    draw: &mut dyn BlockEntityDrawContext,
    host: &dyn ClientHost,
    pos: [i32; 3],
    address: &str,
) -> bool {
    // The crosshair's block as the last client tick saw it (a block entity's drawing is not
    // told of the crosshair).
    if HOVERED.get() != Some(pos) {
        return false;
    }
    let Some(cam) = host.camera() else {
        return false;
    };
    let d2: f64 = (0..3)
        .map(|i| (cam.position[i] - f64::from(pos[i]) - 0.5).powi(2))
        .sum();
    if d2 > 4096.0 {
        return false;
    }
    let text = mod_api::Text::literal(address);
    draw.push();
    draw.translate([0.5, 1.25, 0.5]);
    // `cameraOrientation()`: `rotationYXZ(PI - yaw, -pitch, 0)`.
    draw.rotate_about([0.0, 1.0, 0.0], 180.0 - cam.yaw, [0.0; 3]);
    draw.rotate_about([1.0, 0.0, 0.0], -cam.pitch, [0.0; 3]);
    draw.scale([0.025, -0.025, 0.025]);
    let width = draw.text_width(&text).unwrap_or(0.0);
    let _ = draw.text(&text, [-(width / 2.0).floor(), 0.0], 0xFF_FFFF, false, None);
    draw.pop();
    true
}
