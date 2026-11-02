// A minecart on a block with `rail` (`IBlockExtension.onMinecartPass`): Create's controller rail
// sets its motion by its power and way.
fn minecart_pass(
    pos: horizon_core::position::BlockPos,
    state: horizon_world::registry::BlockStateId,
    cart: &mut crate::game::native_minecarts::NativeMinecart,
) {
    crate::game::create_redstone::minecart_pass([pos.x, pos.y, pos.z], state, cart);
}
