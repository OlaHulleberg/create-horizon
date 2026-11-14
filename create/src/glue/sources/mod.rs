//! What the power sources draw and keep of their own: water wheels' material
//! (`waterwheels`), and the handles of hand cranks and valve handles (`handles`). Their speeds
//! and stress are the network's (`kinetics`); a crank's and a valve's use are `interaction`'s
//! and `blocks`'.

pub mod handles;
pub mod waterwheels;

use super::subsystem::Subsystem;
use create_engine::Block;
use create_engine::client::Draw;
use mod_api::{
    ClientHost, Flow, HostSide, ModBlockEntity, Registrar, RegistrationContext,
    RightClickBlockEvent,
};

/// The block entity data keys the sources' clients read.
const SYNCED: &[&str] = &[
    waterwheels::MATERIAL,
    handles::VALVE_USE,
    handles::SCROLL_VALUE,
];

pub struct Sources;

impl Subsystem for Sources {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        waterwheels::register(r);
        handles::register(r);
    }

    fn synced_keys(&self) -> &'static [&'static str] {
        SYNCED
    }

    fn draws_itself(&self, block: Block, part: &Draw) -> bool {
        waterwheels::draws_itself(block, part) || handles::draws_itself(block, part)
    }

    fn animated(&self, block: Block, pos: [i32; 3], entity: &ModBlockEntity) -> bool {
        handles::animated(block, pos, entity)
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        handles::client_tick(&*host);
    }

    fn client_reset(&self) {
        handles::client_reset();
    }

    fn right_click_block(&self, host: HostSide, event: &mut RightClickBlockEvent<'_>) -> Flow {
        match host {
            HostSide::Client(host) => waterwheels::predict(host, event),
            HostSide::Server(_) => Flow::Continue,
        }
    }
}
