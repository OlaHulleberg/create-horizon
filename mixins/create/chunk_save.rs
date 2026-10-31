// A chunk is saved: Create writes its settings for it.
fn chunk_data_save(dimension: &str, chunk: [i32; 2], data: &mut horizon_world::nbt::Compound) {
    crate::game::create::chunk_data_save(dimension, chunk, data);
}
