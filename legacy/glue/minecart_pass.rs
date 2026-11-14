// A minecart on a block with `rail` (`IBlockExtension.onMinecartPass`): Create's controller rail
// sets its motion by its power and way; a cart assembler takes the cart at its next tick.
fn minecart_pass(
    dimension: &str,
    pos: horizon_core::position::BlockPos,
    state: horizon_world::registry::BlockStateId,
    cart: &mut crate::game::native_minecarts::NativeMinecart,
) {
    crate::game::create_redstone::minecart_pass([pos.x, pos.y, pos.z], state, cart);
    crate::game::create_carts::minecart_pass(dimension, [pos.x, pos.y, pos.z], state, cart);
}
