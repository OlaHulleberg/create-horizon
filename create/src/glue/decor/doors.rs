//! The sliding doors' block entity (`SlidingDoorBlockEntity`) as its clients draw it
//! (`SlidingDoorRenderer`): while a door is not `visible` its block draws nothing (its
//! blockstate has no hidden cases) and the lower half's block entity draws both halves slid
//! aside, or a folding door's two leaves folded, as far as its panel has got. The panel's
//! animation is each client's own (`LerpedFloat.linear`, 0.15 a tick); the server's (`blocks`)
//! only decides when the door shows its block again.

use create_engine::door::{FoldStep, fold_pose, slide_offset};
use create_engine::kinetics::Dir;
use mod_api::{
    BlockEntityDrawContext, BlockEntityPart, BlockEntityRenderBoundingBoxEvent,
    BlockEntitySubmitEvent, BlockKey, BlockPart, ClientHost, HostBlockStates, Registrar,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The sliding doors' block entity type.
pub const TYPE: &str = "create:sliding_door";

/// The doors, and the folding ones' leaves (`AllPartialModels.FOLDING_DOORS`: left, right).
const DOORS: [(&str, Option<[&str; 2]>); 5] = [
    (
        "create:andesite_door",
        Some([
            "create:part_andesite_door_fold_left",
            "create:part_andesite_door_fold_right",
        ]),
    ),
    ("create:brass_door", None),
    (
        "create:copper_door",
        Some([
            "create:part_copper_door_fold_left",
            "create:part_copper_door_fold_right",
        ]),
    ),
    ("create:train_door", None),
    ("create:framed_glass_door", None),
];

/// Each door's block key and its leaves' keys, once registered.
static KEYS: OnceLock<Vec<(BlockKey, Option<[BlockKey; 2]>)>> = OnceLock::new();

pub(super) fn register(r: &mut dyn Registrar) {
    let keys = DOORS
        .iter()
        .map(|(door, leaves)| {
            (
                r.block_key(door),
                leaves.map(|[l, rt]| [r.block_key(l), r.block_key(rt)]),
            )
        })
        .collect();
    let _ = KEYS.set(keys);
    let blocks: Vec<&str> = DOORS.iter().map(|(d, _)| *d).collect();
    let kind = r.block_entity_type(TYPE).blocks(&blocks).build();
    if let Err(e) = r.block_entity_part(&kind, DoorEntity) {
        crate::create_log!("[create] the sliding door block entity's part was refused: {e:?}");
    }
}

/// A client's view of one door's panel: where it was at `since` (game time with the partial
/// tick) and where it is heading.
#[derive(Clone, Copy)]
struct Panel {
    from: f32,
    since: f64,
    target: f32,
}

impl Panel {
    /// `LerpedFloat.getValue` of a linear chase at 0.15 a tick.
    fn value(&self, now: f64) -> f32 {
        let moved = ((now - self.since).max(0.0) * 0.15) as f32;
        if self.target > self.from {
            (self.from + moved).min(self.target)
        } else {
            (self.from - moved).max(self.target)
        }
    }
}

std::thread_local! {
    static PANELS: RefCell<HashMap<[i32; 3], Panel>> = RefCell::new(HashMap::new());
}

struct DoorEntity;

#[mod_api::horizon_part]
impl BlockPart for DoorEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for DoorEntity {
    /// `createRenderBoundingBox().inflate(1)`: the panel reaches into the next block.
    fn block_entity_render_bounding_box(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityRenderBoundingBoxEvent<'_>,
    ) {
        *event.bounds = [-1.0, -1.0, -1.0, 2.0, 3.0, 2.0];
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let facts = host.block_states();
        let prop = |k: &str| {
            event
                .state
                .property(&facts, k)
                .ok()
                .flatten()
                .map(str::to_owned)
        };
        let draw = &mut *event.draw;
        if prop("half").as_deref() != Some("lower") {
            draw.keep();
            return;
        }
        let open = prop("open").as_deref() == Some("true");
        let visible = prop("visible").as_deref() != Some("false");
        let now = draw.game_time() as f64 + f64::from(draw.partial());
        let target = if open { 1.0 } else { 0.0 };
        let value = PANELS.with_borrow_mut(|panels| {
            let panel = panels.entry(event.pos).or_insert(Panel {
                from: target,
                since: now,
                target,
            });
            if panel.target != target {
                *panel = Panel {
                    from: panel.value(now),
                    since: now,
                    target,
                };
            }
            panel.value(now)
        });
        let settled = value == target;
        if visible {
            // `shouldRenderSpecial`: the block draws itself.
            if settled {
                draw.keep();
            }
            return;
        }
        let Some((key, leaves)) = KEYS.get().and_then(|k| {
            DOORS
                .iter()
                .position(|(d, _)| *d == event.block)
                .map(|i| k[i])
        }) else {
            return;
        };
        let facing = prop("facing")
            .as_deref()
            .and_then(super::dir_named)
            .unwrap_or(Dir::North);
        let hinge = prop("hinge").unwrap_or_else(|| "left".into());
        let hinge_right = hinge == "right";
        match leaves {
            Some(leaves) => {
                for left in [true, false] {
                    let (model_left, steps) = fold_pose(facing, hinge_right, left, value);
                    draw.push();
                    for step in steps {
                        match step {
                            FoldStep::Translate(by) => draw.translate(by),
                            FoldStep::TurnY { degrees, pivot } => {
                                draw.rotate_about([0.0, 1.0, 0.0], degrees, pivot)
                            }
                        }
                    }
                    let leaf = if model_left { leaves[0] } else { leaves[1] };
                    draw.block(leaf, &[("facing", "north")]);
                    draw.pop();
                }
            }
            None => {
                let offset = slide_offset(facing, hinge_right, value);
                let facing = super::super::kinetics::dir_name(facing);
                for (half, y) in [("lower", 0.0), ("upper", 1.0 - 1.0 / 512.0)] {
                    draw.push();
                    draw.translate([offset[0], offset[1] + y, offset[2]]);
                    draw_closed(draw, key, facing, half, &hinge);
                    draw.pop();
                }
            }
        }
        if settled {
            draw.keep();
        }
    }
}

/// The door's block, closed and shown (`setValue(OPEN, false)`), as one half.
fn draw_closed(
    draw: &mut dyn BlockEntityDrawContext,
    key: BlockKey,
    facing: &str,
    half: &str,
    hinge: &str,
) {
    draw.block(
        key,
        &[
            ("facing", facing),
            ("half", half),
            ("hinge", hinge),
            ("open", "false"),
            ("powered", "false"),
            ("visible", "true"),
        ],
    );
}

/// Forgets the panels when the client leaves its level.
pub(super) fn forget() {
    PANELS.with_borrow_mut(HashMap::clear);
}
