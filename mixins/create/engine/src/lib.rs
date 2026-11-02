//! Create 6.0.11's rules, translated from its MIT Java into plain Rust with no game types,
//! so every rule is unit-tested here and the game glue (`runtime.rs`) stays thin.

pub mod actors;
pub mod arm;
pub mod basin;
pub mod belt;
pub mod bezier;
pub mod board;
pub mod burner;
pub mod cart;
pub mod chain;
pub mod chain_conveyor;
pub mod chute;
pub mod client;
pub mod clock;
pub mod comparator;
pub mod contraption;
pub mod crafter;
pub mod crusher;
pub mod cuckoo;
pub mod decor;
pub mod deployer;
pub mod depot;
pub mod diode;
pub mod display;
pub mod door;
pub mod drain;
pub mod drill;
pub mod dye;
pub mod ejector;
pub mod elevator;
pub mod equipment;
pub mod fan;
pub mod filter;
pub mod fluids;
pub mod funnel;
pub mod gantry;
pub mod gauge;
pub mod generated;
pub mod glue;
pub mod graph;
pub mod hose;
pub mod kinetics;
pub mod lamp;
pub mod latch;
pub mod lever;
pub mod lifts;
pub mod link;
pub mod network;
pub mod nugget;
pub mod observe;
pub mod package;
pub mod packager;
pub mod parts;
pub mod piston;
pub mod place;
pub mod plumbing;
pub mod port;
pub mod postbox;
pub mod potato;
pub mod press;
pub mod processing;
pub mod psi;
pub mod pulley;
pub mod rail;
pub mod saw;
pub mod schedule;
pub mod schematic;
pub mod schematicannon;
pub mod sequencer;
pub mod spout;
pub mod states;
pub mod steam;
pub mod stock;
pub mod symmetry;
pub mod tank;
pub mod toolbox;
pub mod track;
pub mod track_curves;
pub mod track_render;
pub mod trains;
pub mod tunnel;
pub mod valve;
pub mod vault;
pub mod waterwheel;
pub mod whistle;
pub mod world;
pub mod worldshaper;
pub mod wrench;

pub use generated::Block;

#[cfg(test)]
mod tests {
    use super::Block;

    #[test]
    fn block_names_round_trip_in_registry_order() {
        assert!(
            Block::NAMES.windows(2).all(|w| w[0] < w[1]),
            "sorted as Horizon registers them"
        );
        assert_eq!(Block::from_name("shaft"), Some(Block::Shaft));
        assert_eq!(Block::Shaft.name(), "shaft");
        assert_eq!(Block::from_name("not_a_block"), None);
    }

    #[test]
    fn stress_values_come_from_create_s_registrations() {
        // AllBlocks.java: CREATIVE_MOTOR setCapacity(16384.0), MECHANICAL_PRESS setImpact(8.0),
        // SHAFT setNoImpact(), WATER_WHEEL setCapacity(32).
        assert_eq!(Block::CreativeMotor.capacity(), 16384.0);
        assert_eq!(Block::MechanicalPress.impact(), 8.0);
        assert_eq!(Block::Shaft.impact(), 0.0);
        assert_eq!(Block::WaterWheel.capacity(), 32.0);
    }
}
