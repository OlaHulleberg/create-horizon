# README images

Every image is an in-game capture of the native port in Sunrise (Minecraft 26.3), with the HUD
hidden. The four feature panels were captured on the official 2026-11-17 nightly in a normally
generated seed-1 world. The text and brass rule come from the original panels. Credit line for
the README: *Create's textures and models © the Create Team.*

The scene is built into a sandy, wooded coast. The timber hall has two carrying belts, two
mechanical arms transferring items from a belt to depots, a single shaft driving three presses,
and three mixers making andesite alloy over kindled blaze burners. Copper pipes and single-column
tanks, meshing cogs, nixie tubes, and a powered flap display fill out the floor. The mill has
an eight-bladed windmill and a large water wheel driven by a water race. A copper-and-brass
locomotive hauls a glazed coach and a barrel-and-vault wagon. The three cars ride on six
large bogeys along a stone viaduct linking Foundry and Coast, with a third station at Mill.

## Files

| File | Size | Shows | Goes |
|---|---|---|---|
| `banner.webp` | 1600x600 | The train on the viaduct into the sunset, the factory hall and windmill; title "Create / for Sunrise & Horizon" | Top of the README, `width="100%"` |
| `readme/panel-machines.webp` | 1200x760, 4 s, 16 fps; 1,458,896 bytes | The rebuilt factory hall: carrying belts, arms transferring, presses, mixers on blaze burners and nixie tubes | Feature grid, row 1 left |
| `readme/panel-trains.webp` | 1200x760, 4 s, 16 fps; 1,465,408 bytes | The copper locomotive, coach and cargo wagon on the coastal viaduct | Feature grid, row 1 right |
| `readme/panel-kinetics.webp` | 1200x760, 4 s, 16 fps; 1,426,844 bytes | The eight-bladed windmill against the sunset | Feature grid, row 2 left |
| `readme/panel-world.webp` | 1200x760, 4 s, 16 fps; 1,455,466 bytes | Aerial: the train on the viaduct, the rebuilt hall, the windmill and sea | Feature grid, row 2 right |
| `readme/anim-factory.webp` | 960x540, 3.5 s | The hall at work: both belts carrying, arms swinging, mixers and presses turning | "In motion" section |
| `readme/anim-windmill.webp` | 960x540, 3 s | The windmill turning (seamless loop) | "In motion" section |
| `readme/anim-train-arrival.webp` | 960x540, 2.6 s | The train running in over the viaduct towards the camera | "In motion" section |
| `readme/anim-water-wheel.webp` | 960x540, 3 s | The large water wheel turning in the stream, cogwheels driving the hall | "In motion" section |
| `readme/anim-belt.webp` | 960x540, 3 s | Close-up of ingots under two presses at the old mill | "In motion" section (optional) |
| `readme/sunset-line.webp` | 1600x900 | The banner shot without type | Gallery |
| `readme/approach.webp` | 1600x900 | Along the viaduct: windmill, hall and the train in the distance | Gallery |
| `readme/locomotive.webp` | 1600x900 | The locomotive close, head-on | Gallery |
| `readme/factory-floor.webp` | 1600x900 | A mechanical arm between a belt and two depots, blaze burners behind | Gallery |
| `readme/water-wheel.webp` | 1600x900 | Large water wheel in the stream, beside the hall | Gallery |
| `readme/gearworks.webp` | 1600x900 | Cogwheel cluster and motor driving the press line, the hall and wheel | Gallery |

## Alt text and captions

| File | Alt text | Caption |
|---|---|---|
| `banner.webp` | Create for Sunrise & Horizon: a copper locomotive on a stone viaduct at sunset beside a Create factory and windmill | — |
| `panel-machines.webp` | A Create factory hall with belts, mechanical arms, presses, mixers on blaze burners and nixie tubes | Every machine, natively in Rust (baked in) |
| `panel-trains.webp` | A copper-and-brass locomotive coming off a viaduct towards the camera | Trains, contraptions, logistics (baked in) |
| `panel-kinetics.webp` | An eight-bladed windmill on a tall tower at sunset | Kinetics you can watch (baked in) |
| `panel-world.webp` | Aerial view of a train on a coastal viaduct, a factory hall and a windmill | Your Create world, no Java (baked in) |
| `anim-factory.webp` | Belts, mechanical arms, mixers and presses working in a factory hall | The factory at work |
| `anim-windmill.webp` | Windmill sails turning | Windmill bearing |
| `anim-train-arrival.webp` | A train arriving over a viaduct | A train on its schedule |
| `anim-water-wheel.webp` | A large water wheel turning in a stream | Water wheel and gearing |
| `anim-belt.webp` | Iron ingots on a moving belt under mechanical presses | Presses on a belt line |
| `sunset-line.webp` | A train on a viaduct at sunset beside a factory and windmill | — |
| `approach.webp` | A railway viaduct leading to a factory and windmill | — |
| `locomotive.webp` | A Create locomotive on large bogeys, head-on | — |
| `factory-floor.webp` | A mechanical arm moving items from a belt to depots | — |
| `water-wheel.webp` | A large water wheel in a stream beside a factory | — |
| `gearworks.webp` | Cogwheels and large cogwheels meshing to drive a press line | — |

## Suggested layout

```html
<p align="center"><img src="docs/media/banner.webp" alt="..." width="100%"></p>

<table><tr>
<td width="50%"><img src="docs/media/readme/panel-machines.webp" alt="..."></td>
<td width="50%"><img src="docs/media/readme/panel-trains.webp" alt="..."></td>
</tr><tr>
<td width="50%"><img src="docs/media/readme/panel-kinetics.webp" alt="..."></td>
<td width="50%"><img src="docs/media/readme/panel-world.webp" alt="..."></td>
</tr></table>
```

Then the four main animations as a second 2x2 table (factory, train, windmill, water wheel), then
the gallery 2 per row, ending with `<sub>` lines naming the Sunrise build and the credit line.

## Design notes

- References: Sunrise's README (a 1600x600 banner at `width="100%"`, 2-up panels in a table, a
  `<sub>` line of context), plus Create's own pages, Sodium, Iris, Bevy and Ghostty (one banner or
  logo and no screenshots). What the image-led READMEs do well: one strong hero, one aspect ratio
  per row, very few words, opaque images that read the same on light and dark themes, WebP at a
  sensible size.
- Banner: 8:3, the shape of Sunrise's, so the two read as one family. The left third is sea and
  sunset sky under a left-to-right andesite gradient, which keeps the title calm; the train,
  hall and windmill fill the right. Fira Sans Heavy for "Create", SemiBold brass for
  "for Sunrise & Horizon", a tracked condensed kicker and a short brass rule.
- Palette from Create's materials: andesite greys (#1E1F22 band), brass (#E2AC52, rule #966828),
  cream type (#F6EEDE), muted grey for subtitles. No transparency, so no light/dark variants.
- Panels: 1200x760 animated WebP. A 1200x610 in-game crop sits over the original
  150 px caption band with its brass rule, headline and subtitle. The first frame holds the
  complete composition for still previews. The train uses a tighter crop around the
  locomotive and hall. Later frames update only the game scene.
- Capture: Sunrise nightly `2d81263`, instance `create-nightly-c` on display `:93`. The camera
  uses the normal 70° FOV with gamma at 1 and 12-chunk render distance. Daylight and weather
  progression were disabled with the 26.3 gamerules before setting the time to 12200 with clear weather. The west-facing
  angles show the sunset. The machines and rotor close-up were captured at 8 ticks/s, the
  train approach at 1.5 ticks/s, and the aerial at 2 ticks/s.
- Camera: cosine-eased translations aimed at fixed subjects. Machines drift 1.05 blocks east,
  0.12 up, and 0.42 south along the open shopfront. The train camera pulls back 1.32 blocks east,
  0.12 up, and 0.25 north. The windmill shot moves 1.12 blocks east, 0.25 up, and 0.68 north.
  The aerial rises 0.45 blocks while moving 1.10 east and 0.85 north. A two-degree yaw pan was
  also tried for each panel. The selected dollies show more depth between the moving subjects
  and their backgrounds.
- Loops: 33 captured frames forward, followed by 31 in reverse. Alternating 62/63 ms holds
  yield four seconds at 16 fps. The camera and machine motion retrace their path, including
  the train approach. The moving images are filtered through 640x325 for the factory and
  800x407 for the other panels before resizing to 1200x610. The first image keeps a sharper
  full-size crop. The caption is pixel-identical across the loop and to the original still.
  Each file stays under 1.5 MB.
- Rebuild and capture scripts, raw frames and review sheets are retained under
  `/mnt/lanes/agent-media/`. See `scene/rebuild.sh`, `capture-coast.py`, `encode-coast.py`,
  `camera-notes-coast.md` and `review/compare-*.png`. The compositions use the rendered game
  frames and caption bands.
- Gallery and animations: 16:9 with no text. Animations are 960x540 at about 16 fps and at most
  1.35 MB each. They were captured with the game's tick rate slowed (`/tick rate 8`; windmill
  `4`), because screenshots arrive at about 6 per second. The windmill clip loops on a full
  symmetry period.

## Limits

- New animated panel captures are 1280x720, cropped and resized to the scene portion.
  Original captures are 1910x1027, the offscreen window's real size: `--width/--height 2560x1440` were
  ignored and the game has no render scale. Enough for the 1600-wide banner, not true 2x.
- Rendering bug: a fluid tank with a 2x2 footprint draws only its top layer; the bottom layer is
  invisible although its block states are right (`bottom=true,shape=window_nw`...).
  Single-column tanks render correctly and are what the shots use; the steam engine boiler was
  left out for this reason.
- Not investigated: an andesite funnel set with `/setblock` past a belt's end, on a barrel
  (`facing=west,extracting=false`), did not take items; the shots end belts on depots or let
  arms take from them.

- A campfire stayed at the locomotive's assembly point during a moving trial. Create's Java
  registers `CampfireMovementBehaviour` to emit smoke at the moving campfire's position.
  The capture locomotive uses a blackstone stack.
- Arm setup used targeted block interactions to select the belt and depot before placing
  each arm. The selection summaries showed one input and one output, and both depots
  received items. Create's Java uses the same point-selection and placement sequence.
