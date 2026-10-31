// A chunk is saved: Create writes its settings and moving parts' state for it.
fn chunk_data_save(
    game: &crate::game::Game<'_>,
    dimension: &str,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    crate::game::create::chunk_data_save(game, dimension, chunk, data);
}
