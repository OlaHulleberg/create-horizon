// A chunk is saved: Create writes its settings and moving parts' state for it, and its backtanks'.
fn chunk_data_save(
    game: &crate::game::Game<'_>,
    dimension: &str,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    crate::game::create::chunk_data_save(game, dimension, chunk, data);
    crate::game::create_equipment::chunk_data_save(dimension, chunk, data);
    crate::game::create_plumbing::chunk_data_save(dimension, chunk, data);
    crate::game::create_packages::chunk_data_save(dimension, chunk, data);
    crate::game::create_schematics::chunk_data_save(game, dimension, chunk, data);
    crate::game::create_decor::chunk_data_save(dimension, chunk, data);
}
