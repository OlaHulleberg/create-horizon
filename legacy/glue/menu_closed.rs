// A mod's menu closed: a postbox whose last menu it was shuts.
fn menu_closed(uuid: u128, window: i32) {
    crate::game::create_blocks::menu_closed(uuid, window);
}
