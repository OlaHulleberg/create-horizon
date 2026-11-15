//! The glue's world pipelines: textured quads drawn with a sprite of Create's
//! (`assets/create/shaders/pipeline/textured.wgsl`, written by `create-data prepare` with the
//! sprites it puts on the block atlas). Each pipeline samples one sprite, tinted by the quad's
//! colour and repeating past a UV of 1, as catnip's `outlineTranslucent(texture)` draws an
//! outline's faces. Optional: without the prepared shader they resolve to nothing and the
//! callers draw untextured quads.

use mod_api::{
    ClientHost, Identifier, PipelineBlend, PipelineTexture, RegisterRenderPipelinesEvent,
    RenderPipeline, RenderPipelineRef, WorldDepth,
};

/// A sprite-textured pipeline of the glue's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Textured {
    /// `AllSpecialTextures.CHECKERED`: the worldshaper's and other outlines' faces.
    Checkered,
    /// `AllSpecialTextures.GLUE`: super glue areas' faces.
    Glue,
}

impl Textured {
    const ALL: [Textured; 2] = [Textured::Checkered, Textured::Glue];

    fn id(self) -> &'static str {
        match self {
            Textured::Checkered => "create:checkered",
            Textured::Glue => "create:glue",
        }
    }

    fn sprite(self) -> &'static str {
        match self {
            Textured::Checkered => "create:block/special/checkerboard",
            Textured::Glue => "create:block/special/glue",
        }
    }
}

/// `register_render_pipelines`: the glue's pipelines, translucent and hidden by nearer blocks.
pub fn register(event: &mut RegisterRenderPipelinesEvent<'_>) {
    let Ok(shader) = Identifier::parse("create:textured") else {
        return;
    };
    for t in Textured::ALL {
        let (Ok(id), Ok(sprite)) = (Identifier::parse(t.id()), Identifier::parse(t.sprite()))
        else {
            continue;
        };
        event.pipelines.push(
            RenderPipeline::new(
                id,
                shader.clone(),
                PipelineBlend::Translucent,
                WorldDepth::Tested,
            )
            .textured(PipelineTexture::Sprite(sprite))
            .optional(),
        );
    }
}

/// The pipeline `t` as the current resources built it; None when they couldn't.
pub fn resolve(host: &dyn ClientHost, t: Textured) -> Option<RenderPipelineRef> {
    let id = Identifier::parse(t.id()).ok()?;
    host.render_assets().render_pipeline(&id).ok()
}
