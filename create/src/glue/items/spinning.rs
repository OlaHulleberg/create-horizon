//! Wrench, potato cannon and worldshaper item models. Their original jar parts draw under the
//! shell's display transform; only the moving part takes Create's rotation about its pivot.

use crate::glue::pipelines::Pose;
use create_engine::equipment::ToolSpin;
use mod_api::{
    ClientHost, ItemStack, SpecialModelContext, SpecialModelPart, SpecialModelPolicy,
    SpecialModelRenderer, SpecialModelRenderers, Tag,
};
use std::sync::Arc;

struct Spinner {
    spin: ToolSpin,
    fixed: &'static [&'static str],
    moving: &'static str,
}

pub(super) fn register(renderers: &mut dyn SpecialModelRenderers) {
    for (id, renderer) in [
        (
            "create:wrench",
            Spinner {
                spin: ToolSpin::Wrench,
                fixed: &["create:item/wrench/item"],
                moving: "create:item/wrench/gear",
            },
        ),
        (
            "create:potato_cannon",
            Spinner {
                spin: ToolSpin::PotatoCannon,
                fixed: &["create:item/potato_cannon/item"],
                moving: "create:item/potato_cannon/cog",
            },
        ),
        (
            "create:handheld_worldshaper",
            Spinner {
                spin: ToolSpin::Worldshaper,
                fixed: &[
                    "create:item/handheld_worldshaper/item",
                    "create:item/handheld_worldshaper/core",
                    "create:item/handheld_worldshaper/core_glow",
                ],
                moving: "create:item/handheld_worldshaper/accelerator",
            },
        ),
    ] {
        renderers.register(id.into(), SpecialModelPolicy::Frame, Arc::new(renderer));
    }
}

impl SpecialModelRenderer for Spinner {
    fn extract(
        &self,
        host: &dyn ClientHost,
        _stack: &ItemStack,
        _config: &Tag,
        _context: &SpecialModelContext,
    ) -> Result<Vec<SpecialModelPart>, String> {
        let (degrees, axis, pivot) = self.spin.pose(host.millis() as f64 / 1000.0);
        let pose = Pose::at([0.0; 3]).rotate(axis, degrees, pivot);
        let mut parts: Vec<_> = self
            .fixed
            .iter()
            .copied()
            .map(SpecialModelPart::new)
            .collect();
        parts.push(SpecialModelPart::new(self.moving).with_transform(pose.affine()));
        Ok(parts)
    }
}
