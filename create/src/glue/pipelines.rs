//! The glue's world pipelines: textured quads drawn with a sprite of Create's
//! (`assets/create/shaders/pipeline/textured.wgsl`, written by `create-data prepare` with the
//! sprites it puts on the block atlas). Each pipeline samples one sprite, tinted by the quad's
//! colour and repeating past a UV of 1, as catnip's `outlineTranslucent(texture)` draws an
//! outline's faces; the glows' add to the frame (`RenderTypes.additive()`). [`Pose`] places a
//! model's faces as a `PoseStack` would, for what a block entity's renderer draws in its pose. Optional: without the prepared shader they resolve to nothing and the
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
    /// `AllIcons.ICON_ATLAS` (`textures/gui/icons.png`): value boxes' frames and icons.
    Icons,
    /// The display link's bulb glow (`DISPLAY_LINK_GLOW`, `link_details`), added to the frame.
    LinkGlow,
    /// A nixie signal's glows and tubes (`SIGNAL_*_GLOW`, `SIGNAL_*` tubes), added to the frame.
    SignalGlow,
    /// The yellow light's glow and tube (`signal_glow_2`), added to the frame.
    SignalGlow2,
    /// A nixie signal's white cube (`SIGNAL_WHITE_CUBE`), translucent.
    SignalCube,
    /// The steam jet's frames side by side (`particles/steam_jet.json`), cut out.
    SteamJet,
}

impl Textured {
    const ALL: [Textured; 8] = [
        Textured::Checkered,
        Textured::Glue,
        Textured::Icons,
        Textured::LinkGlow,
        Textured::SignalGlow,
        Textured::SignalGlow2,
        Textured::SignalCube,
        Textured::SteamJet,
    ];

    fn id(self) -> &'static str {
        match self {
            Textured::Checkered => "create:checkered",
            Textured::Glue => "create:glue",
            Textured::Icons => "create:icons",
            Textured::LinkGlow => "create:link_glow",
            Textured::SignalGlow => "create:signal_glow",
            Textured::SignalGlow2 => "create:signal_glow_2",
            Textured::SignalCube => "create:signal_cube",
            Textured::SteamJet => "create:steam_jet",
        }
    }

    fn sprite(self) -> &'static str {
        match self {
            Textured::Checkered => "create:block/special/checkerboard",
            Textured::Glue => "create:block/special/glue",
            Textured::Icons => "create:block/special/gui_icons",
            Textured::LinkGlow => "create:block/link_details",
            Textured::SignalGlow | Textured::SignalCube => "create:block/signal_glow",
            Textured::SignalGlow2 => "create:block/signal_glow_2",
            Textured::SteamJet => "create:block/special/steam_jet",
        }
    }

    /// `RenderTypes.additive()` for the glows, else translucent.
    fn blend(self) -> PipelineBlend {
        match self {
            Textured::LinkGlow | Textured::SignalGlow | Textured::SignalGlow2 => {
                PipelineBlend::Additive
            }
            _ => PipelineBlend::Translucent,
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
            RenderPipeline::new(id, shader.clone(), t.blend(), WorldDepth::Tested)
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

/// An affine transform of points, built as a `PoseStack` is: each call applies to what is
/// drawn after it, in the space the calls before it made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    m: [[f64; 4]; 3],
}

impl Pose {
    /// The transform that moves points by `at` (a block's corner in the world).
    pub fn at(at: [f64; 3]) -> Pose {
        Pose {
            m: [
                [1.0, 0.0, 0.0, at[0]],
                [0.0, 1.0, 0.0, at[1]],
                [0.0, 0.0, 1.0, at[2]],
            ],
        }
    }

    fn mul(self, o: [[f64; 4]; 3]) -> Pose {
        let mut m = [[0.0; 4]; 3];
        for (r, row) in m.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| self.m[r][k] * o[k][c]).sum::<f64>()
                    + if c == 3 { self.m[r][3] } else { 0.0 };
            }
        }
        Pose { m }
    }

    pub fn translate(self, v: [f64; 3]) -> Pose {
        self.mul([
            [1.0, 0.0, 0.0, v[0]],
            [0.0, 1.0, 0.0, v[1]],
            [0.0, 0.0, 1.0, v[2]],
        ])
    }

    pub fn scale(self, v: [f64; 3]) -> Pose {
        self.mul([
            [v[0], 0.0, 0.0, 0.0],
            [0.0, v[1], 0.0, 0.0],
            [0.0, 0.0, v[2], 0.0],
        ])
    }

    /// `Axis.rotationDegrees`: `degrees` about unit axis `axis` (counterclockwise seen from
    /// its positive end), through `pivot`.
    pub fn rotate(self, axis: [f64; 3], degrees: f64, pivot: [f64; 3]) -> Pose {
        let (s, c) = degrees.to_radians().sin_cos();
        let [x, y, z] = axis;
        let t = 1.0 - c;
        let r = [
            [t * x * x + c, t * x * y - s * z, t * x * z + s * y, 0.0],
            [t * x * y + s * z, t * y * y + c, t * y * z - s * x, 0.0],
            [t * x * z - s * y, t * y * z + s * x, t * z * z + c, 0.0],
        ];
        self.translate(pivot).mul(r).translate(pivot.map(|v| -v))
    }

    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let m = &self.m;
        [0, 1, 2].map(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
    }

    /// Face `side` (`Direction` order: down, up, north, south, west, east) of the box
    /// `from`..`to` (sixteenths), placed by this pose, in `color`, with the sprite's `uv`
    /// (sixteenths) turned by `rotation` degrees as a baked model face's are.
    #[allow(clippy::too_many_arguments)]
    pub fn face(
        &self,
        from: [f64; 3],
        to: [f64; 3],
        side: usize,
        uv: [f32; 4],
        rotation: u32,
        color: u32,
        out: &mut Vec<mod_api::PipelineQuad>,
    ) {
        let corners = face_corners(from.map(|v| v / 16.0), to.map(|v| v / 16.0), side);
        let uvs = face_uvs(uv, rotation);
        out.push(mod_api::PipelineQuad::new(
            corners.map(|c| self.apply(c)),
            uvs,
            color,
        ));
    }
}

/// The corners of face `side` (`Direction` order) of the box `from`..`to`: top left, bottom
/// left, bottom right, top right seen from outside, the order a baked model face's are in.
pub fn face_corners<T: Copy>(from: [T; 3], to: [T; 3], side: usize) -> [[T; 3]; 4] {
    // Per corner, whether each axis takes `to` (else `from`).
    let pick: [[bool; 3]; 4] = match side {
        0 => [
            [false, false, true],
            [false, false, false],
            [true, false, false],
            [true, false, true],
        ],
        1 => [
            [false, true, false],
            [false, true, true],
            [true, true, true],
            [true, true, false],
        ],
        2 => [
            [true, true, false],
            [true, false, false],
            [false, false, false],
            [false, true, false],
        ],
        3 => [
            [false, true, true],
            [false, false, true],
            [true, false, true],
            [true, true, true],
        ],
        4 => [
            [false, true, false],
            [false, false, false],
            [false, false, true],
            [false, true, true],
        ],
        _ => [
            [true, true, true],
            [true, false, true],
            [true, false, false],
            [true, true, false],
        ],
    };
    pick.map(|c| std::array::from_fn(|i| if c[i] { to[i] } else { from[i] }))
}

/// A face's `uv` rectangle (sixteenths) at its corners (in units of the sprite), turned by
/// `rotation` degrees: corner `i` takes the rectangle's corner `i` plus a quarter turn per 90.
pub fn face_uvs(uv: [f32; 4], rotation: u32) -> [[f32; 2]; 4] {
    let [u0, v0, u1, v1] = uv.map(|v| v / 16.0);
    let rect = [[u0, v0], [u0, v1], [u1, v1], [u1, v0]];
    let turn = (rotation / 90) as usize;
    std::array::from_fn(|i| rect[(i + turn) % 4])
}

#[cfg(test)]
mod tests {
    use super::{Pose, face_corners, face_uvs};

    #[test]
    fn a_turned_face_starts_its_uv_a_corner_on() {
        let uv = [0.0, 0.0, 16.0, 8.0];
        assert_eq!(
            face_uvs(uv, 0),
            [[0.0, 0.0], [0.0, 0.5], [1.0, 0.5], [1.0, 0.0]]
        );
        assert_eq!(
            face_uvs(uv, 90),
            [[0.0, 0.5], [1.0, 0.5], [1.0, 0.0], [0.0, 0.0]]
        );
        // The up face's top left corner is its north-west one.
        assert_eq!(face_corners([0, 0, 0], [1, 1, 1], 1)[0], [0, 1, 0]);
    }

    #[test]
    fn a_pose_turns_about_its_pivot_then_moves() {
        // A quarter turn about y through a block's centre takes its north-west corner east.
        let p = Pose::at([10.0, 0.0, 0.0]).rotate([0.0, 1.0, 0.0], 90.0, [0.5; 3]);
        let q = p.apply([0.0, 0.0, 0.0]);
        let want = [10.0, 0.0, 1.0];
        assert!((0..3).all(|i| (q[i] - want[i]).abs() < 1e-9), "{q:?}");
    }
}
