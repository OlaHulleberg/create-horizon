//! `AllFluids.registerFluidInteractions`: source lava becomes obsidian; flowing lava
//! becomes limestone beside honey and scoria beside chocolate. The host applies these
//! declarations on placement and neighbour changes, including the fizz.

use mod_api::{FluidBecomes, FluidInteraction, FluidNeighbour, PlacedState, Registrar};

pub(super) fn register(r: &mut dyn Registrar) {
    let lava = r.fluid_key("minecraft:lava");
    let obsidian = r.block_key("minecraft:obsidian");
    for (fluid, block) in [
        ("create:honey", "create:limestone"),
        ("create:chocolate", "create:scoria"),
    ] {
        let neighbour = FluidNeighbour::Fluid(r.fluid_key(fluid));
        let mut becomes = FluidBecomes::default();
        becomes.source = Some(PlacedState::of(obsidian));
        becomes.flowing = Some(PlacedState::of(r.block_key(block)));
        r.fluid_interaction(FluidInteraction::new(lava, neighbour, becomes));
    }
}
