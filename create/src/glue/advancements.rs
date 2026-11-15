//! Create's advancements (`AllAdvancements`, `CreateAdvancement`): the files are Create's data
//! generator's (`data/create/advancement`, converted by `create-data advancements`), each
//! completed by its built-in trigger `create:<name>_builtin` (`SimpleCreateTrigger`), which
//! `award` fires where Create calls `awardTo`. The item advancements (`inventory_changed`) are
//! the game's own. The game gives the toast, the chat line and the advancement screen's tab.

use mod_api::{
    CriterionContext, CriterionTest, CriterionTestError, Identifier, PlayerRef, ServerHost,
};
use std::sync::Arc;

/// `SimpleCreateTrigger`'s test: a built-in trigger has no conditions.
struct Always;

impl CriterionTest for Always {
    fn test(&self, _criterion: &dyn CriterionContext) -> Result<bool, CriterionTestError> {
        Ok(true)
    }
}

/// `CreateAdvancement.awardTo`: the advancement `name` (`belt`, `potato_cannon`) for `player`.
pub(crate) fn award(host: &mut dyn ServerHost, player: PlayerRef, name: &str) {
    let Ok(trigger) = Identifier::parse(&format!("create:{name}_builtin")) else {
        return;
    };
    if let Err(e) = player.trigger_criterion(host, &trigger, Arc::new(Always)) {
        crate::create_log!("[create] the {name} advancement's trigger was refused: {e:?}");
    }
}
