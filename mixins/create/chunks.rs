// A column joined a server level: Create finds its blocks there.
fn chunk_load(
    game: &mut crate::game::Game<'_>,
    dimension: &str,
    chunk: &horizon_client::chunk::Chunk,
) {
    crate::game::create::chunk_load(game, dimension, chunk);
}
