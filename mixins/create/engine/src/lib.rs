//! Create 6.0.11's rules, translated from its MIT Java into plain Rust with no game types,
//! so every rule is unit-tested here and the game glue (`runtime.rs`) stays thin.

pub mod generated;
pub mod states;

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
}
