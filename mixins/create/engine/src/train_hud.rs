//! The train HUD while a player controls a carriage (`TrainHUD`): the speed bar over the
//! hotbar, the throttle bar with its pointer (the mouse wheel sets the throttle,
//! `TrainHUD.onScroll`, sent as `TrainHUDUpdatePacket.Serverbound`), and the prompt box above
//! them (`TrainPromptPacket`: a station arrived at or departed, the station ahead, an
//! approach's progress). Its numbers chase what the server last said
//! (`TrainHUDUpdatePacket.Clientbound`) as Catnip's `LerpedFloat` with `Chaser.EXP` does
//! (`value += (target - value) * speed` each tick, drawn between the last two values). Drawn
//! from Create's `widgets` GUI texture (`AllGuiTextures.TRAIN_HUD_*`, `TRAIN_PROMPT*`), read
//! from the player's jar, and the direction arrow from Catnip's placement indicator sheet
//! (Ponder's MIT repository, committed with the content mod).

/// The GUI texture the HUD's parts come from (`create:textures/gui/widgets.png`).
pub const TEXTURE: &str = "create:gui/widgets";
/// Catnip's placement indicator sheet (`PlacementClient.textured`): 16 frames of 16 x 16 down a
/// 16 x 256 texture, one per 22.5 degrees.
pub const ARROW_TEXTURE: &str = "create:gui/placement_indicator";
/// Throttle steps (`TrainHUD.onScroll`: 1/18 a notch, from 1/18 to 1).
pub const THROTTLE_STEPS: u8 = 18;
/// `TrainPromptPacket.handle`: ticks a prompt stays without another.
pub const PROMPT_KEEP_ALIVE: u8 = 30;
/// `CarriageContraptionEntity.hudPacketCooldown` and `TrainHUD.hudPacketCooldown`: ticks
/// between HUD words.
pub const PACKET_COOLDOWN: u8 = 5;
/// The prompt's text colour (`TrainHUD.renderOverlay`).
pub const PROMPT_TEXT: u32 = 0x54_4D_45;

/// A part of the texture: x, y, width, height (`AllGuiTextures`).
pub type Part = [f32; 4];
pub const SPEED_BG: Part = [0.0, 190.0, 182.0, 5.0];
pub const SPEED: Part = [0.0, 185.0, 182.0, 5.0];
pub const THROTTLE: Part = [0.0, 195.0, 182.0, 5.0];
pub const THROTTLE_POINTER: Part = [0.0, 209.0, 6.0, 9.0];
pub const FRAME: Part = [0.0, 200.0, 186.0, 7.0];
pub const DIRECTION: Part = [77.0, 165.0, 28.0, 20.0];
pub const PROMPT_L: Part = [8.0, 209.0, 3.0, 16.0];
pub const PROMPT_R: Part = [11.0, 209.0, 3.0, 16.0];
pub const PROMPT: Part = [0.0, 230.0, 256.0, 16.0];

/// Catnip's `LerpedFloat.linear()` chased by `Chaser.EXP`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lerped {
    previous: f32,
    value: f32,
}

impl Lerped {
    /// `chase(target, speed, EXP)` then `tickChaser`.
    pub fn chase(&mut self, target: f32, speed: f32) {
        self.previous = self.value;
        self.value += (target - self.value) * speed;
    }

    /// `getValue(partialTicks)`.
    pub fn get(&self, partial: f32) -> f32 {
        self.previous + (self.value - self.previous) * partial
    }
}

/// The HUD's state on the client while it controls a carriage (`TrainHUD`'s statics): what
/// the server last said (the speed as a share of the manual top speed, the throttle), the
/// throttle being set by the wheel and not yet confirmed (`editedThrottle`), the chased
/// values, and the prompt's keep-alive (the prompt's text is the glue's).
#[derive(Clone, Debug, PartialEq)]
pub struct Hud {
    pub speed: f32,
    pub throttle: f32,
    pub edited: Option<f32>,
    pub shown_speed: Lerped,
    pub shown_throttle: Lerped,
    pub prompt_size: Lerped,
    pub keep_alive: u8,
    cooldown: u8,
}

impl Default for Hud {
    fn default() -> Self {
        Hud {
            speed: 0.0,
            throttle: 1.0,
            edited: None,
            shown_speed: Lerped::default(),
            shown_throttle: Lerped {
                previous: 1.0,
                value: 1.0,
            },
            prompt_size: Lerped::default(),
            keep_alive: 0,
            cooldown: PACKET_COOLDOWN,
        }
    }
}

impl Hud {
    /// `TrainPromptPacket.handle`: a prompt came; it stays 30 ticks.
    pub fn prompted(&mut self) {
        self.keep_alive = PROMPT_KEEP_ALIVE;
    }

    /// `TrainHUD.tick` with the prompt's width in GUI pixels while it has one (None once its
    /// keep-alive ran out: the glue lets its text go). Returns the throttle to send the server
    /// (`TrainHUDUpdatePacket.Serverbound`), in steps of 1/18, every 5 ticks until it agrees.
    pub fn tick(&mut self, prompt_width: Option<f32>) -> Option<u8> {
        self.keep_alive = self.keep_alive.saturating_sub(1);
        let prompt = prompt_width.filter(|_| self.keep_alive > 0);
        self.prompt_size
            .chase(prompt.map_or(0.0, |w| w + 17.0), 0.5);
        let value = (self.speed + 0.05).clamp(0.0, 1.0);
        self.shown_speed.chase((value * 18.0).floor() / 18.0, 0.5);
        self.shown_throttle
            .chase(self.edited.unwrap_or(self.throttle), 0.75);
        let edited = self.edited?;
        if (edited - self.throttle).abs() < 1.0e-5 {
            self.edited = None;
            self.cooldown = PACKET_COOLDOWN;
            return None;
        }
        if self.cooldown == 0 {
            self.cooldown = PACKET_COOLDOWN;
            return Some(steps(edited));
        }
        self.cooldown -= 1;
        None
    }

    /// Whether the prompt is still shown (its keep-alive not run out).
    pub fn prompt_alive(&self) -> bool {
        self.keep_alive > 0
    }

    /// `TrainHUD.onScroll`: a notch of the wheel moves the throttle by 1/18, between 1/18 and 1.
    pub fn scroll(&mut self, delta: f32) {
        let previous = self.edited.unwrap_or(self.throttle);
        let notch = if delta > 0.0 { 1.0 } else { -1.0 } / 18.0;
        self.edited = Some((previous + notch).clamp(1.0 / 18.0, 1.0));
    }
}

/// A throttle in 1/18 steps (1 to 18).
pub fn steps(throttle: f32) -> u8 {
    (throttle * f32::from(THROTTLE_STEPS))
        .round()
        .clamp(1.0, f32::from(THROTTLE_STEPS)) as u8
}

/// The throttle of `steps` 1/18 steps.
pub fn throttle(steps: u8) -> f64 {
    f64::from(steps.clamp(1, THROTTLE_STEPS)) / f64::from(THROTTLE_STEPS)
}

/// What the HUD draws: texture parts at GUI rectangles (x, y, width, height), and the prompt's
/// text at its top left when it fits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawn {
    pub parts: Vec<(Part, [f32; 4])>,
    pub text: Option<[f32; 2]>,
}

/// `TrainHUD.renderOverlay`'s layout on a `gui` sized screen (GUI pixels), `partial` ticks
/// into the frame, the prompt `text_width` wide (if any): the frame, the speed bar filled to
/// the speed, the prompt box grown to its size, the direction plate, the throttle bar filled
/// from its right and its pointer. Not drawn: the direction arrow over the plate (Catnip's
/// `PlacementClient.textured`, whose texture is not Create's).
pub fn layout(hud: &Hud, gui: [f32; 2], partial: f32, text_width: Option<f32>) -> Drawn {
    let ox = (gui[0] as i32 / 2 - 91) as f32;
    let oy = gui[1] - 29.0;
    let mut d = Drawn::default();
    let mut put = |part: Part, x: f32, y: f32| {
        d.parts.push((part, [ox + x, oy + y, part[2], part[3]]));
    };
    put(FRAME, -2.0, 1.0);
    put(SPEED_BG, 0.0, 0.0);
    let w = (SPEED[2] * hud.shown_speed.get(partial)).floor();
    if w > 0.0 {
        put([SPEED[0], SPEED[1], w, SPEED[3]], 0.0, 0.0);
    }
    let size = hud.prompt_size.get(partial).floor();
    if size > 1.0 {
        let px = size / -2.0 + 91.0;
        put(PROMPT_L, px - 3.0, -27.0);
        put(PROMPT_R, px + size, -27.0);
        let u = PROMPT[0] + (128.0 - size / 2.0);
        put([u, PROMPT[1], size, PROMPT[3]], px, -27.0);
        if let Some(tw) = text_width.filter(|&tw| tw < size - 10.0) {
            d.text = Some([ox + tw / -2.0 + 82.0 + 9.0, oy - 27.0 + 4.0]);
        }
    }
    put(DIRECTION, 77.0, -20.0);
    let w = (THROTTLE[2] * (1.0 - hud.shown_throttle.get(partial))).floor();
    let inv = THROTTLE[2] - w;
    if w > 0.0 {
        put([THROTTLE[0] + inv, THROTTLE[1], w, THROTTLE[3]], inv, 0.0);
    }
    put(THROTTLE_POINTER, (THROTTLE[2] - w).max(1.0) - 3.0, -2.0);
    d
}

/// Catnip's `AngleHelper.getShortestAngleDiff`: from `current` to `target`, -180 to 180.
fn shortest_angle_diff(current: f64, target: f64) -> f32 {
    let (current, target) = (current % 360.0, target % 360.0);
    ((((target - current) % 360.0) + 540.0) % 360.0 - 180.0) as f32
}

/// `TrainHUD.renderOverlay`'s direction: the way the train goes, seen from the camera, snapped
/// to 22.5 degrees. `camera_yaw` is the camera entity's `getYRot`, `carriage_yaw` the carriage
/// entity's `yaw`; `inverted` when the controls face away from the carriage's
/// `getInitialOrientation` counterclockwise; `pressed` the controls held (bit per control):
/// back reverses it, left and right turn it 45 degrees.
pub fn arrow_angle(camera_yaw: f32, carriage_yaw: f32, inverted: bool, pressed: u8) -> f32 {
    use crate::train_controls::{BACK, LEFT, RIGHT};
    let held = |k: u8| pressed & (1 << k) != 0;
    let reversing = held(BACK);
    let inverted = inverted ^ reversing;
    let mut offset = if held(LEFT) { -45 } else { 0 } + if held(RIGHT) { 45 } else { 0 };
    if reversing {
        offset = -offset;
    }
    let snap = 22.5f32;
    let mut diff = shortest_angle_diff(f64::from(camera_yaw), f64::from(carriage_yaw))
        + if inverted { -90.0 } else { 90.0 };
    if diff.abs() < 60.0 {
        diff = 0.0;
    }
    let angle = diff + offset as f32;
    // Java's `Math.round(float)`: the floor of x + 0.5.
    (snap * (angle / snap + 0.5).floor()) % 360.0
}

/// The arrow over the direction plate for `angle` (`arrow_angle`) on a `gui` sized screen: its
/// frame of `ARROW_TEXTURE` and where it goes. The pose is `translate(91, -9)` and
/// `scale(0.925)` from the HUD's corner, then Catnip's `translate(0, 0, 50)`,
/// `scale(indicatorScale * 0.75 * 12)` (the scale's default, 1) over a quad from -1 to 1. The
/// frame is the angle over 22.5 down the sheet, wrapping as the texture repeats.
pub fn arrow(gui: [f32; 2], angle: f32) -> (Part, [f32; 4]) {
    let ox = (gui[0] as i32 / 2 - 91) as f32;
    let oy = gui[1] - 29.0;
    let half = 0.925 * 0.75 * 12.0;
    let frame = ((angle / 22.5) as i32).rem_euclid(16) as f32;
    (
        [0.0, frame * 16.0, 16.0, 16.0],
        [ox + 91.0 - half, oy - 9.0 - half, half * 2.0, half * 2.0],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_speed_bar_chases_the_speed_in_eighteenths() {
        let mut h = Hud {
            speed: 0.5,
            ..Hud::default()
        };
        // 0.55 of the way: the bar aims at 9/18.
        for _ in 0..40 {
            h.tick(None);
        }
        assert!((h.shown_speed.get(1.0) - 0.5).abs() < 1.0e-4);
        // Half way from the last value to the target each tick.
        let mut h = Hud {
            speed: 1.0,
            ..Hud::default()
        };
        h.tick(None);
        assert!((h.shown_speed.get(1.0) - 0.5).abs() < 1.0e-6);
        assert!((h.shown_speed.get(0.5) - 0.25).abs() < 1.0e-6);
    }

    #[test]
    fn the_wheel_sets_the_throttle_and_tells_the_server_until_it_agrees() {
        let mut h = Hud::default();
        // Full throttle: a notch up stays at 1, two down make 16/18.
        h.scroll(1.0);
        assert_eq!(h.edited, Some(1.0));
        h.scroll(-1.0);
        h.scroll(-1.0);
        assert_eq!(h.edited.map(steps), Some(16));
        // Sent after the cooldown (5 ticks), then every 5 more until the server says so.
        let sent: Vec<Option<u8>> = (0..12).map(|_| h.tick(None)).collect();
        assert_eq!(sent.iter().flatten().count(), 2);
        assert_eq!(sent[5], Some(16));
        assert_eq!(sent[11], Some(16));
        h.throttle = throttle(16) as f32;
        assert_eq!(h.tick(None), None);
        assert_eq!(h.edited, None);
        // Never below 1/18.
        for _ in 0..40 {
            h.scroll(-1.0);
        }
        assert_eq!(h.edited.map(steps), Some(1));
    }

    #[test]
    fn a_prompt_lives_30_ticks() {
        let mut h = Hud::default();
        h.prompted();
        for _ in 0..29 {
            h.tick(Some(40.0));
        }
        assert!(h.prompt_alive());
        // Grown toward its text's width and 17.
        assert!((h.prompt_size.get(1.0) - 57.0).abs() < 1.0e-3);
        h.tick(Some(40.0));
        assert!(!h.prompt_alive());
    }

    #[test]
    fn the_layout_is_create_s() {
        let mut h = Hud {
            speed: 1.0,
            ..Hud::default()
        };
        for _ in 0..60 {
            h.tick(None);
        }
        // 427 x 240 GUI: the bars start at 213 - 91 = 122, 29 over the bottom.
        let d = layout(&h, [427.0, 240.0], 1.0, None);
        assert_eq!(d.parts[0], (FRAME, [120.0, 212.0, 186.0, 7.0]));
        assert_eq!(d.parts[1], (SPEED_BG, [122.0, 211.0, 182.0, 5.0]));
        // Full speed: the speed bar full; full throttle: no throttle bar, the pointer at 179.
        assert_eq!(d.parts[2].1, [122.0, 211.0, 182.0, 5.0]);
        assert_eq!(d.parts[3], (DIRECTION, [199.0, 191.0, 28.0, 20.0]));
        assert_eq!(d.parts[4], (THROTTLE_POINTER, [301.0, 209.0, 6.0, 9.0]));
        assert_eq!(d.parts.len(), 5);
        // Half throttle: the bar's right half from x 91, the pointer at 88.
        h.throttle = 0.5;
        for _ in 0..60 {
            h.tick(None);
        }
        let d = layout(&h, [427.0, 240.0], 1.0, None);
        assert_eq!(
            d.parts[4],
            ([91.0, 195.0, 91.0, 5.0], [213.0, 211.0, 91.0, 5.0])
        );
        assert_eq!(d.parts[5].1, [210.0, 209.0, 6.0, 9.0]);
        // A prompt 40 wide: a 57 wide box centred over the bars (the middle part from the
        // texture's centre), its text inside.
        h.prompted();
        for _ in 0..25 {
            h.tick(Some(40.0));
        }
        let d = layout(&h, [427.0, 240.0], 1.0, Some(40.0));
        let size = 57.0;
        let px = 122.0 + size / -2.0 + 91.0;
        assert!(
            d.parts.contains(&(PROMPT_L, [px - 3.0, 184.0, 3.0, 16.0])),
            "{d:?}"
        );
        assert!(d.parts.contains(&(PROMPT_R, [px + size, 184.0, 3.0, 16.0])));
        assert_eq!(d.text, Some([122.0 - 20.0 + 82.0 + 9.0, 188.0]));
    }

    #[test]
    fn the_arrow_points_the_way_the_train_goes_from_the_camera() {
        use crate::train_controls::{BACK, LEFT, RIGHT};
        // shortest(camera -> carriage) + 90, zero inside 60, snapped to 22.5.
        // Camera at the carriage's yaw + 90: straight ahead; 20 degrees off is in the dead zone.
        assert_eq!(arrow_angle(180.0, 90.0, false, 0), 0.0);
        assert_eq!(arrow_angle(160.0, 90.0, false, 0), 0.0);
        // Along the carriage: a quarter turn; -30 + 90 = 60 is out of the zone: 67.5.
        assert_eq!(arrow_angle(90.0, 90.0, false, 0), 90.0);
        assert_eq!(arrow_angle(120.0, 90.0, false, 0), 67.5);
        // Inverted controls take 90 instead; reversing inverts too.
        assert_eq!(arrow_angle(0.0, 90.0, true, 0), 0.0);
        assert_eq!(arrow_angle(0.0, 90.0, false, 1 << BACK), 0.0);
        // Left and right turn it 45 degrees, mirrored while reversing.
        assert_eq!(arrow_angle(180.0, 90.0, false, 1 << LEFT), -45.0);
        assert_eq!(arrow_angle(180.0, 90.0, false, 1 << RIGHT), 45.0);
        assert_eq!(arrow_angle(0.0, 90.0, false, 1 << BACK | 1 << LEFT), 45.0);
    }

    #[test]
    fn the_arrow_sits_over_the_plate_one_frame_per_step() {
        // 427 x 240: the corner at (122, 211); the plate is drawn at (77, -20) from it, 28 wide.
        let (src, dst) = arrow([427.0, 240.0], 45.0);
        assert_eq!(src, [0.0, 32.0, 16.0, 16.0]);
        let half = 0.925 * 0.75 * 12.0;
        assert_eq!(dst, [213.0 - half, 202.0 - half, 2.0 * half, 2.0 * half]);
        // A negative angle wraps down the repeating sheet: -22.5 is the last frame.
        assert_eq!(arrow([427.0, 240.0], -22.5).0[1], 15.0 * 16.0);
    }
}
