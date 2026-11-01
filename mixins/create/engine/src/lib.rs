//! Create 6.0.11's rules, translated from its MIT Java into plain Rust with no game types,
//! so every rule is unit-tested here and the game glue (`runtime.rs`) stays thin.

pub mod basin;
pub mod belt;
pub mod burner;
pub mod chute;
pub mod client;
pub mod contraption;
pub mod crusher;
pub mod depot;
pub mod drill;
pub mod fan;
pub mod fluids;
pub mod generated;
pub mod kinetics;
pub mod latch;
pub mod parts;
pub mod piston;
pub mod place;
pub mod press;
pub mod processing;
pub mod saw;
pub mod spout;
pub mod states;
pub mod tank;
pub mod trains;
pub mod waterwheel;
pub mod world;
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
