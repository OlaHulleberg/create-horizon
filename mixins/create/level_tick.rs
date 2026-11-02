// After a server level's tick: Create's backtanks fill, then Create re-solves what changed there
// (broken blocks' filter items drop), seats seat the players who clicked them and go riderless;
// last, comparators beside blocks whose output changed read them again.
fn level_tick_post(game: &mut crate::game::Game<'_>, dimension: &str) {
    crate::game::create_equipment::level_tick(game, dimension);
    crate::game::create::level_tick_post(game, dimension);
    crate::game::create_packages::level_tick(game, dimension);
    crate::game::create_stock::level_tick(game, dimension);
    crate::game::create_schematics::level_tick(game, dimension);
    crate::game::create_decor::level_tick(game, dimension);
    crate::game::create_seats::level_tick(game, dimension);
    crate::game::create_items::level_tick(game, dimension);
    crate::game::create::tick_comparators(game, dimension);
}
