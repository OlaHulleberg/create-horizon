// A column left a server level: Create forgets its blocks there.
fn chunk_unload(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    crate::game::create::chunk_unload(dimension, chunk);
    crate::game::create_packages::chunk_unload(dimension, chunk);
}
