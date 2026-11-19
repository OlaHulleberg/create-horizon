# README images

Every image is an in-game capture of this port running in Sunrise (Minecraft 26.3), HUD hidden,
seed 1, just before sunset. Every machine shown is running (belts, presses, mixers, arms, wheels,
the windmill, the train on its schedule). Nothing is painted over; the banner and panels only add
a gradient and type. Credit line for the README: *Create's textures and models © the Create Team.*

The scene: a factory hall on the coast (two belts at different levels crossing, two mechanical
arms sorting onto depots, presses on a line shaft, mixers over basins on lit blaze burners,
meshing cogwheels and large cogwheels, gearboxes, brass-encased shafts, pipes and single-column
fluid tanks, a display board and nixie tubes). Behind it, an older mill with a large water wheel
in a stream and an eight-bladed windmill on a tall tower. In front, a stone viaduct over the sea
where a copper-and-brass locomotive on large bogeys pulls a coach and a cargo wagon (barrels,
item vaults) between three stations.

The four feature panels come from a newer scene, built on the same seed at spawn (the sand bay
west of the forest): an open-fronted timber hall under a copper-shingle roof (three belts on the
floor and one under the tie beams, presses and deployers on line shafts, mixers on lit blaze
burners meshed by a cog row, millstones, mechanical crafters, large cogs on the back wall and the
overhead shaft, a display board reading "SUNRISE WORKS / NATIVE RUST - NO JAVA"), a big gear on a
mechanical bearing turning on the gable, a works yard with two mechanical arms, nixie tubes and a
chain conveyor loop on four spinning poles carrying packages between two packager/frogport
stations, a windmill on the point, three large water wheels under a flume, a brick chimney, a
slewing harbour crane on a girder tower (geared down to 4 rpm), and the viaduct with a
locomotive, coach and cargo wagon between the Works and Pier stations.

## Files

| File | Size | Shows | Goes |
|---|---|---|---|
| `banner.webp` | 1600x600 | The train on the viaduct into the sunset, the factory hall and windmill; title "Create / for Sunrise & Horizon" | Top of the README, `width="100%"` |
| `readme/panel-machines.avif` | 1200x760, 8 s, 20 fps | Arc over the works yard into the hall: belts at two levels carrying, arms sorting onto depots, presses, deployers, mixers on lit blaze burners, a chain conveyor carrying packages between two frogports, the display board | Feature grid, row 1 left |
| `readme/panel-trains.avif` | 1200x760, 8 s, 20 fps | Arc round the locomotive standing at the Pier station on the viaduct, the slewing crane, windmill and gable rotor turning behind | Feature grid, row 1 right |
| `readme/panel-kinetics.avif` | 1200x760, 8 s, 20 fps | Arc round the eight-bladed windmill turning against the sunset, the sea and white cliffs below | Feature grid, row 2 left |
| `readme/panel-world.avif` | 1200x760, 8 s, 20 fps | Aerial arc over the coast: the train leaving the Works station along the viaduct, the crane, the hall, windmill, water wheels and chimney | Feature grid, row 2 right |
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
| `banner.webp` | Create for Sunrise: a copper locomotive on a stone viaduct at sunset beside a Create factory and windmill | — |
| `panel-machines.avif` | A Create factory hall with belts, mechanical arms, presses, mixers on blaze burners, a chain conveyor carrying packages and a display board | Every machine, natively in Rust (baked in) |
| `panel-trains.avif` | A copper-and-brass locomotive on large bogeys on a stone viaduct, a harbour crane and windmill behind | Trains, contraptions, logistics (baked in) |
| `panel-kinetics.avif` | An eight-bladed windmill turning on a tall tower against the sunset | Kinetics you can watch (baked in) |
| `panel-world.avif` | Aerial view of a coastal Create works: a train leaving along the viaduct, a slewing crane, the hall, a windmill and water wheels | Your Create world, no Java (baked in) |
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
<td width="50%"><img src="docs/media/readme/panel-machines.avif" alt="..."></td>
<td width="50%"><img src="docs/media/readme/panel-trains.avif" alt="..."></td>
</tr><tr>
<td width="50%"><img src="docs/media/readme/panel-kinetics.avif" alt="..."></td>
<td width="50%"><img src="docs/media/readme/panel-world.avif" alt="..."></td>
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
- Panels: all 1200x760 (~1.58:1) with one 150 px andesite caption band and a brass rule, one
  headline and one short line, so they pair in 2-up rows and scale down cleanly.
- Gallery and animations: 16:9 with no text. Animations are 960x540 at about 16 fps and at most
  1.35 MB each. They were captured with the game's tick rate slowed (`/tick rate 8`; windmill
  `4`), because screenshots arrive at about 6 per second. The windmill clip loops on a full
  symmetry period.
- Feature panels: animated AVIF (SVT-AV1, CRF 24-27, preset 4, yuv420p, looping), about 1.5-2 MB
  each. Each is a slow arc: the camera circles a fixed focus at constant distance and eye height,
  15 degrees in all (azimuth `c - 7.5 cos(2 pi i / N)`, so it eases at both ends and returns), over
  160 frames shown at 20 fps (8 s). The game is frozen (`/tick freeze`) and stepped one tick per
  frame (`/tick step 1`), so machines move forward a tick between frames while only the camera
  swings back. Captured at 2400x1220 and downscaled exactly 2x; the caption band is the old still's
  bottom 150 px pasted onto every frame before encoding. Daylight frozen at `/time set 12100`,
  FOV 70.
- Golden hour is `/time set 12000`–`12200` with gamma at max, so the machines stay readable
  against the sky.

## Limits

- The older captures are 1910x1027. On the 2d81263 nightly, `--width 2400 --height 1220` with an
  Xvfb screen larger than the window gives true 2400x1220 frames, used for the feature panels.
- Not investigated: three times the integrated server dropped ("disconnected: End of stream",
  no crash report, panic or error in any log) in a frozen take stepped one tick at a time: twice
  with the train running under its schedule, once in the hall with about 60 items on belts. A
  retake worked each time. The trains panel shows the train standing at the Pier station (a
  running train would also jump where the 8 s loop wraps).
- Rendering bug: a fluid tank with a 2x2 footprint draws only its top layer; the bottom layer is
  invisible although its block states are right (`bottom=true,shape=window_nw`...).
  Single-column tanks render correctly and are what the shots use; the steam engine boiler was
  left out for this reason.
- Not investigated: an andesite funnel set with `/setblock` past a belt's end, on a barrel
  (`facing=west,extracting=false`), did not take items; the shots end belts on depots or let
  arms take from them.
