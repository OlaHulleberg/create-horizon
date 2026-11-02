// A menu opens for a block: a toolbox lays its compartments out.
fn toolbox_screen(block: &str, screen: &mut Option<crate::game::native_hooks::NativeScreen>) {
    crate::game::create::screen(block, screen);
}
