//! What the power sources draw and keep of their own: water wheels' material
//! (`waterwheels`), and the handles of hand cranks and valve handles (`handles`); what
//! clients see of a network's stress changing (`stress`); and what they hear of turning
//! machinery (`ambience`). Their speeds and stress are the
//! network's (`kinetics`); a crank's and a valve's use are `interaction`'s and `blocks`'.

pub mod ambience;
pub mod handles;
pub mod indicators;
pub mod stress;
pub mod waterwheels;

use super::server::ServerLevel;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::client::Draw;
use create_engine::dye;
use mod_api::{
    ClientHost, Compound, EntityPlaceEvent, EquipmentSlot, Flow, HostSide, ModBlockEntity,
    ParticleProviders, Registrar, RegistrationContext, RightClickBlockEvent, ServerHost, Tag,
};
use std::cell::RefCell;
use std::sync::Arc;

/// The block entity data keys the sources' clients read.
const SYNCED: &[&str] = &[
    waterwheels::MATERIAL,
    handles::VALVE_USE,
    handles::SCROLL_VALUE,
    GENERATED,
];

/// The speed a source generates (`getGeneratedSpeed`) in its block entity data, for the goggles
/// (`GeneratingKineticBlockEntity.addToGoggleTooltip` counts its capacity at that speed).
pub const GENERATED: &str = "Gen";

thread_local! {
    /// Sources placed or loaded since the last level tick, by level.
    static PLACED: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

/// Whether `block` generates (`GeneratingKineticBlockEntity`).
fn generates(block: Block) -> bool {
    block.capacity() > 0.0 || block == Block::PoweredShaft || dye::is_valve_handle(block)
}

/// After the solve: each source whose settings changed, or that came into the level, tells its
/// clients the speed it generates now.
fn tell_generated(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut due =
        with_world(|w| std::mem::take(&mut w.dims[dim].generator_changes)).unwrap_or_default();
    PLACED.with_borrow_mut(|p| {
        p.retain(|(d, pos)| {
            if *d == dim {
                due.push(*pos);
            }
            *d != dim
        })
    });
    if due.is_empty() {
        return;
    }
    due.sort();
    due.dedup();
    for pos in due {
        let Some(generated) = with_world(|w| {
            let (block, _) = w.table.get(w.state_at(dim, pos)?)?;
            generates(block).then(|| w.dims[dim].kinetics.generated(pos, 0).unwrap_or(0.0))
        })
        .flatten() else {
            continue;
        };
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let told = entity
            .data(&*lv.host)
            .and_then(|d| match d.get(&GENERATED.into()) {
                Some(Tag::Float(bits)) => Some(f32::from_bits(*bits)),
                _ => None,
            });
        // `updateGeneratedRotation`: a source that drives its network (`!hasSource`) and
        // changes speed level shows its indicators.
        let level = create_engine::gauge::SpeedLevel::of;
        let driving = with_world(|w| w.dims[dim].kinetics.speed(pos, 0))
            .flatten()
            .is_some_and(|s| s.abs() == generated.abs());
        if level(told.unwrap_or(0.0)) != level(generated) && driving {
            indicators::queue(dim, pos);
        }
        if told != Some(generated) {
            let data = Compound::from([(GENERATED.into(), Tag::Float(generated.to_bits()))]);
            let _ = entity.merge_data(&mut *lv.host, data);
        }
    }
}

/// The speed the source whose block entity data is `data` generates, as its clients last
/// heard.
pub fn generated_of(data: &Compound) -> Option<f32> {
    match data.get(&GENERATED.into()) {
        Some(Tag::Float(bits)) => Some(f32::from_bits(*bits)),
        _ => None,
    }
}

pub struct Sources;

impl Subsystem for Sources {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        waterwheels::register(r);
        handles::register(r);
        stress::register(r);
        ambience::register(r);
        indicators::register(r);
    }

    fn synced_keys(&self) -> &'static [&'static str] {
        SYNCED
    }

    fn block_changed(&self, dim: usize, pos: [i32; 3], _old: u32, new: u32) {
        let source = with_world(|w| w.table.get(new).is_some_and(|(b, _)| generates(b)));
        if source == Some(true) {
            PLACED.with_borrow_mut(|p| p.push((dim, pos)));
        }
    }

    fn neighbor_changed(&self, dim: usize, pos: [i32; 3], state: u32) {
        handles::neighbor_changed(dim, pos, state);
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tell_generated(lv);
        handles::level_tick(lv);
        indicators::level_tick(lv);
    }

    fn draws_itself(&self, block: Block, part: &Draw) -> bool {
        waterwheels::draws_itself(block, part) || handles::draws_itself(block, part)
    }

    fn animated(&self, block: Block, pos: [i32; 3], entity: &ModBlockEntity) -> bool {
        handles::animated(block, pos, entity)
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        handles::client_tick(&*host);
        stress::client_tick(host);
        ambience::client_tick(host);
        let goggles = host.local_player().is_some_and(|p| {
            let head = host
                .living(p.entity())
                .and_then(|l| l.equipment(&*host, EquipmentSlot::Head));
            super::items::is(head.as_ref(), super::items::Item::Goggles)
        });
        indicators::client_tick(goggles);
    }

    fn particle_providers(&self, providers: &mut dyn ParticleProviders) {
        providers.register(indicators::NAME.to_owned(), Arc::new(indicators::Indicator));
    }

    /// `KineticBlock.setPlacedBy`: a kinetic block a player places shows its indicators.
    fn entity_place(&self, host: &mut dyn ServerHost, event: &mut EntityPlaceEvent) -> Flow {
        let Some(placer) = event.placer else {
            return Flow::Continue;
        };
        let Some(dim) = placer
            .level(&*host)
            .and_then(|l| l.dimension().id(&*host))
            .and_then(|d| super::world::level_index(&d))
        else {
            return Flow::Continue;
        };
        let kinetic = super::states::id(event.state)
            .and_then(|id| with_world(|w| w.table.is_kinetic(id)))
            == Some(true);
        if kinetic {
            indicators::queue(dim, [event.pos.x, event.pos.y, event.pos.z]);
        }
        Flow::Continue
    }

    fn client_reset(&self) {
        handles::client_reset();
        stress::client_reset();
        ambience::client_reset();
    }

    fn right_click_block(&self, host: HostSide, event: &mut RightClickBlockEvent<'_>) -> Flow {
        match host {
            HostSide::Client(host) => waterwheels::predict(host, event),
            HostSide::Server(_) => Flow::Continue,
        }
    }
}
