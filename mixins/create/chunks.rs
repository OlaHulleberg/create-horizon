// A column joined a server level: Create finds its blocks there, and its backtanks' air.
fn chunk_load(
    game: &mut crate::game::Game<'_>,
    dimension: &str,
    chunk: &horizon_client::chunk::Chunk,
) {
    // Before the blocks: what valves' pipes, hoses and creative tanks saved.
    crate::game::create_plumbing::chunk_load(dimension, chunk);
    crate::game::create::chunk_load(game, dimension, chunk);
    crate::game::create_equipment::chunk_load(dimension, chunk);
    crate::game::create_packages::chunk_load(dimension, chunk);
    crate::game::create_stock::chunk_load(game, dimension, chunk);
    crate::game::create_schematics::chunk_load(game, dimension, chunk);
    crate::game::create_decor::chunk_load(dimension, chunk);
    crate::game::create_controller::chunk_load(dimension, chunk);
}
