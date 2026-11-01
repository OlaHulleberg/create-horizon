#!/usr/bin/env python3
"""Tick and frame times of the built game for one Create scenario, through /workspace/headless.sh.

Each run starts the game invisibly (Xvfb, --offscreen, scratch homes) from a scratch folder, checks that
the renderer is the discrete GPU (not llvmpipe), builds the scenario with commands, holds the camera
over it and then flies a fixed path with `horizon-ctl fly ... raw`, which reports every frame time and
the server thread's smoothed tick time. Scenarios:

  off      no content mod (Horizon as it is)
  idle     the Create content mod on, no Create block in the world
  static   the content mod on, a 32x32 floor of Create decoration blocks in view (phase 1)
  kinetic  the content mod on, 64 creative motors each turning a line of 63 shafts: 4,096 turning
           parts in view, steady (phase 2)
  chains   the control for kinetic: the same layout in vanilla iron blocks and iron chains
  processing  16 rows of a creative motor turning 16 mechanical presses, each over a depot of 64
           iron ingots: 256 presses pressing at 16 rpm (phase 3)
  logistics  32 belts 48 long at 16 rpm, each carrying 21 stacks dropped on it: 672 stacks moving
           (phase 4)
  redstone 32 observer clocks, each driving a chain of 16 toggle latches (a binary counter): 512
           latches switching (phase 7)
  repeaters  the control for redstone: the same chains of vanilla repeaters
  diodes   the same chains of Create's pulse extenders (512 brass diodes passing the clocks' pulses on)
  tunnels  the logistics belts with 21 andesite tunnels over each (segments 10 to 30, windowed
           straight tunnels): five stacks dropped on the first 9 segments of each pass under them (phase 4)
  belts9   the control for tunnels: the same belts and stacks, no tunnels
  deployeruse 16 deployers at 16 rpm facing east, each given a water bucket by a player's click: each
           stroke pours the water out or takes it back up (`native_use_item_at`, phase 3)
  deployerstroke the same 16 deployers given a stick instead: strokes that use nothing, the control
           for `deployeruse` (its water placed and taken up remeshes chunks: vanilla's cost)
  waterchurn the vanilla control for `deployeruse`: 16 dispensers with a water bucket each where the
           deployers' hands reach from, fired every 4 s (a deployer at 16 rpm: 80 ticks a use)
  waterstill the same 16 water sources set once and left to spread: the water sheet's own drawing
  honey    16 honey sources (Create's fluid, gap 5) set once and left to spread: against waterstill
  curvetracks 32 track blocks in pairs, a quarter turn apart (control for curves)
  curves   the same pairs joined by 16 curved tracks placed with the track item (GPU parts)
  gauges   the kinetic rows with speedometers and stressometers in place of shafts (4,032 gauges whose
           needles settle and are drawn still)
  deployers  16 deployers at 16 rpm facing down over depots, each given 64 oak planks by a
           player's click and 64 shafts on its depot: 16 deployers making cogwheels (phase 3)
  bearings 16 mechanical bearings facing up at 16 rpm, each turning a slime block with four
           stones stuck to it (16 structures drawn as one GPU part each, phase 8)
  chassis  the pistons scene pushing a linear chassis glued on top under 3 stones in place of each
           stone (16 structures of 4 blocks gliding, phase 8)
  gantries 16 lines of 30 gantry shafts at 16 rpm, a carriage on each carrying a slime block and a
           stone, started as the flight starts (16 structures gliding, phase 8)
  gantryshafts  the control for gantries: the same shaft lines turning, no carriages
  crafters 16 pairs of mechanical crafters at 16 rpm (cogs between them), each fed coal from a
           hopper above and sticks from a hopper beside, crafting torches into a chest below
           (phase 3)
  brass    16 belts 30 long at 16 rpm, each with a brass tunnel over segment 15 between two depots,
           5 stacks of 8 iron dropped on the first 9 segments: each stack split among the
           depots and the belt ahead (phase 4)
  brassbelts  the control for brass: the same belts, depots and stacks, no tunnels
  signals  the trains scene with two track signals by each track (green: no other train on it):
           each train looks for red signals ahead every tick (phase 9)
  links    32 observer clocks, each pulsing a redstone link 16 blocks from a receiving one that
           lights a lamp; all untuned, so one network of 64 links changes at every pulse (phase 7)
  contraptions  16 windmills (17 sails each, a cross) assembled and turning: 16 structures drawn as
           one GPU part each (phase 8)
  crushers 30 pairs of crushing wheels at 16 rpm (z axis, turning against each other), each fed a
           stack of 16 gravel: 30 controllers crushing (phase 3)
  mixers   32 basins of 64 andesite and 64 iron nuggets under mechanical mixers at 16 rpm (each
           driven by a cog on a motor): 32 mixers making andesite alloy (phase 3)
  pumps    10 mechanical pumps at 16 rpm, each pulling water from an infinite pool through pipes
           into a fluid tank (phase 5)
  trains   12 straight tracks of 57 with a station at each end and a train (a bogey, a slime block
           and a gold block) assembled at the first, all sent to the far station as the
           measured flight starts (phase 9)
  pistons  16 mechanical pistons with 8 poles each pushing a stone, started by motors as the
           measured flight starts (16 structures gliding, phase 8)
  goggles  the kinetic layout in mechanical presses with Engineer's Goggles worn, the overlay
           drawn for the press under the crosshair every frame (phase 6)
  presses  its control: the same presses, no goggles
  chutes   16 columns of 8 chutes, each fed by a hopper of 64 iron above and emptying into a
           chest below (phase 4)
  drills   32 drills at 16 rpm, each before a stone (broken during the settle; then each looks
           again every 10 ticks) (phase 3)
  saws     32 saws facing up at 16 rpm, each given 64 andesite alloy to cut as the flight starts
           (phase 3)
  spouts   10 pumps filling spouts from pools, each over a depot of 16 empty buckets (phase 5)
  funnels  32 chests of 27 stacks each emptied by an extracting funnel, and 32 inserting
           funnels idle on chests (phase 4)
  chests   the funnels' control: the same chests and a stack lying in front of each, no funnels
  drains   10 drains emptying a water bucket each into pumps carrying it to spouts (phase 5)
  burners  a 32x32 floor of blaze burners with their blaze (1,024 passive burners, the head in the
           chunk mesh): no tick (phase 3)
  fans     32 encased fans at 16 rpm blowing through water over three depots each of 64 ice: 96
           stacks washed into packed ice (phase 3)

    python3 tools/bench_create.py --game target/horizon-cargo/fast/horizon-client \\
        --content target/create-content --scenario idle --repeats 3 --out benchmarks/p0-idle.json
"""
import argparse
import json
import os
import re
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HEADLESS = Path("/workspace/headless.sh")
# The scene is built this far above the spawn point, in loaded chunks.
LIFT = 30
STATIC_BLOCKS = ["andesite_casing", "brass_casing", "copper_casing", "cut_andesite_bricks",
                 "polished_cut_crimsite", "layered_veridium", "framed_glass", "copper_shingles"]


def ctl(ctl_bin, sock, *args, timeout=600):
    out = subprocess.run([str(ctl_bin), "--socket", sock, "--timeout", str(timeout), *args],
                         capture_output=True, text=True, timeout=timeout + 30)
    if out.returncode:
        raise RuntimeError(f"horizon-ctl {' '.join(args)}: {out.stderr.strip()}")
    return json.loads(out.stdout) if out.stdout.strip().startswith("{") else out.stdout


def command(ctl_bin, sock, line, failures):
    chat = ctl(ctl_bin, sock, "command", line).get("chat", [])
    bad = [c for c in chat if not c.startswith(("Changed the block", "Successfully filled", "No blocks were"))]
    if bad:
        failures.append((line, bad))


def build(ctl_bin, sock, scenario, center):
    """Builds the scene; returns the commands that failed (an unloaded position, a limit)."""
    x, y, z = center
    failures = []
    # Clear layer by layer: one fill may change at most 32,768 blocks.
    for yy in range(y - 7, y + 4):
        command(ctl_bin, sock, f"fill {x-32} {yy} {z-32} {x+32} {yy} {z+32} air", failures)
    if scenario == "static":
        # 32x32 floor, one block kind per 4-wide stripe: 1024 Create blocks in view.
        for i, name in enumerate(STATIC_BLOCKS):
            x0 = x - 16 + i * 4
            command(ctl_bin, sock, f"fill {x0} {y-5} {z-16} {x0+3} {y-5} {z+15} create:{name}", failures)
    if scenario == "gauges":
        for row in range(64):
            zz = z - 32 + row
            gauge = "speedometer" if row % 2 == 0 else "stressometer"
            command(ctl_bin, sock, f"setblock {x-32} {y-6} {zz} create:creative_motor[facing=east]", failures)
            command(ctl_bin, sock, f"fill {x-31} {y-6} {zz} {x+31} {y-6} {zz} "
                    f"create:{gauge}[facing=up,axis_along_first=true]", failures)
    if scenario in ("kinetic", "chains", "goggles", "presses"):
        # `chains`, the control: the same layout in vanilla blocks (the shaft's template, an iron chain).
        motor, shaft = {
            "kinetic": ("create:creative_motor[facing=east]", "create:shaft[axis=x]"),
            "chains": ("minecraft:iron_block", "minecraft:iron_chain[axis=x]"),
        }.get(scenario, ("create:creative_motor[facing=east]", "create:mechanical_press[facing=east]"))
        for row in range(64):
            zz = z - 32 + row
            command(ctl_bin, sock, f"setblock {x-32} {y-6} {zz} {motor}", failures)
            command(ctl_bin, sock, f"fill {x-31} {y-6} {zz} {x+31} {y-6} {zz} {shaft}", failures)
    if scenario == "processing":
        for row in range(16):
            zz = z - 16 + 2 * row
            command(ctl_bin, sock, f"setblock {x-17} {y-4} {zz} create:creative_motor[facing=east]", failures)
            command(ctl_bin, sock, f"fill {x-16} {y-4} {zz} {x-1} {y-4} {zz} create:mechanical_press[facing=east]",
                    failures)
            # Placed holding their stack, as a saved depot loads (one by one: Horizon's `fill`
            # drops block entity data for mod blocks).
            for i in range(16):
                command(ctl_bin, sock, f"setblock {x-16+i} {y-6} {zz} "
                        'create:depot{Items:[{Slot:0b,id:"minecraft:iron_ingot",count:64}]}', failures)
    if scenario == "mixers":
        for col in range(4):
            for row in range(8):
                x0, zz = x - 16 + 6 * col, z - 16 + 4 * row
                command(ctl_bin, sock, f"setblock {x0} {y-6} {zz} create:basin"
                        '{Items:[{Slot:0b,id:"minecraft:andesite",count:64},'
                        '{Slot:1b,id:"minecraft:iron_nugget",count:64}]}', failures)
                command(ctl_bin, sock, f"setblock {x0} {y-4} {zz} create:mechanical_mixer", failures)
                command(ctl_bin, sock, f"setblock {x0+1} {y-4} {zz} create:cogwheel[axis=y]", failures)
                command(ctl_bin, sock, f"setblock {x0+1} {y-5} {zz} create:creative_motor[facing=up]", failures)
    if scenario == "pumps":
        for row in range(10):
            zz, x0 = z - 15 + 3 * row, x
            command(ctl_bin, sock, f"fill {x0-7} {y-7} {zz-1} {x0+4} {y-7} {zz+2} stone", failures)
            command(ctl_bin, sock, f"fill {x0-6} {y-6} {zz-1} {x0-2} {y-6} {zz+2} glass", failures)
            command(ctl_bin, sock, f"fill {x0-5} {y-6} {zz} {x0-3} {y-6} {zz+1} water", failures)
            command(ctl_bin, sock, f"fill {x0-2} {y-6} {zz} {x0-1} {y-6} {zz} create:fluid_pipe", failures)
            command(ctl_bin, sock, f"setblock {x0} {y-6} {zz} create:mechanical_pump[facing=east]", failures)
            command(ctl_bin, sock, f"fill {x0+1} {y-6} {zz} {x0+2} {y-6} {zz} create:fluid_pipe", failures)
            command(ctl_bin, sock, f"setblock {x0+3} {y-6} {zz} create:fluid_tank", failures)
            command(ctl_bin, sock, f"setblock {x0} {y-5} {zz} create:cogwheel[axis=x]", failures)
            command(ctl_bin, sock, f"setblock {x0+1} {y-5} {zz} create:creative_motor[facing=west]", failures)
    if scenario in ("boilers", "tanks"):
        # Pool -> pump at 32 rpm (a large cogwheel turning its small cog) -> a 2x2 tank over four
        # kindled blaze burners; "boilers" adds a steam engine on its south side driving a shaft.
        for row in range(10):
            zz, x0 = z - 30 + 6 * row, x
            command(ctl_bin, sock, f"fill {x0-7} {y-7} {zz-1} {x0+6} {y-7} {zz+5} stone", failures)
            command(ctl_bin, sock, f"fill {x0-6} {y-6} {zz-1} {x0-2} {y-6} {zz+2} glass", failures)
            command(ctl_bin, sock, f"fill {x0-5} {y-6} {zz} {x0-3} {y-6} {zz+1} water", failures)
            command(ctl_bin, sock, f"fill {x0-2} {y-6} {zz} {x0-1} {y-6} {zz} create:fluid_pipe", failures)
            command(ctl_bin, sock, f"setblock {x0} {y-6} {zz} create:mechanical_pump[facing=east]", failures)
            command(ctl_bin, sock, f"fill {x0+1} {y-6} {zz} {x0+2} {y-6} {zz} create:fluid_pipe", failures)
            command(ctl_bin, sock, f"fill {x0+3} {y-7} {zz} {x0+4} {y-7} {zz+1} create:blaze_burner[blaze=kindled]",
                    failures)
            command(ctl_bin, sock, f"fill {x0+3} {y-6} {zz} {x0+4} {y-6} {zz+1} create:fluid_tank", failures)
            command(ctl_bin, sock, f"setblock {x0} {y-5} {zz+1} create:large_cogwheel[axis=x]", failures)
            command(ctl_bin, sock, f"setblock {x0+1} {y-5} {zz+1} create:creative_motor[facing=west]", failures)
            command(ctl_bin, sock, f"setblock {x0+4} {y-6} {zz+4} create:shaft[axis=x]", failures)
            if scenario == "boilers":
                command(ctl_bin, sock, f"setblock {x0+4} {y-6} {zz+2} "
                        "create:steam_engine[face=wall,facing=south,waterlogged=false]", failures)
    if scenario in ("trains", "signals"):
        for row in range(12):
            zz = z - 24 + 4 * row
            command(ctl_bin, sock, f"fill {x-28} {y-6} {zz} {x+28} {y-6} {zz} create:track[shape=xo]", failures)
            command(ctl_bin, sock, f"setblock {x-27} {y-6} {zz-1} create:track_station", failures)
            command(ctl_bin, sock, f"setblock {x+27} {y-6} {zz-1} create:track_station", failures)
            command(ctl_bin, sock, f"setblock {x-26} {y-5} {zz} create:small_bogey[axis=x]", failures)
            command(ctl_bin, sock, f"setblock {x-26} {y-4} {zz} minecraft:slime_block", failures)
            command(ctl_bin, sock, f"setblock {x-26} {y-3} {zz} minecraft:gold_block", failures)
            if scenario == "signals":
                command(ctl_bin, sock, f"setblock {x-10} {y-6} {zz-1} create:track_signal", failures)
                command(ctl_bin, sock, f"setblock {x+10} {y-6} {zz-1} create:track_signal", failures)
    if scenario in ("goggles", "presses"):
        # A floor of presses just under the flight, in reach of the crosshair: the overlay shows
        # the one looked at (not counted: the count looks lower).
        command(ctl_bin, sock, f"fill {x-14} {y-1} {z-14} {x+14} {y-1} {z+14} create:mechanical_press[facing=east]",
                failures)
    if scenario == "chutes":
        for i in range(16):
            xx, zz = x - 12 + 6 * (i % 4), z - 12 + 6 * (i // 4)
            command(ctl_bin, sock, f"setblock {xx} {y-7} {zz} minecraft:chest", failures)
            command(ctl_bin, sock, f"fill {xx} {y-6} {zz} {xx} {y+1} {zz} create:chute", failures)
            command(ctl_bin, sock, f"setblock {xx} {y+2} {zz} minecraft:hopper[facing=down]"
                    '{Items:[{Slot:0b,id:"minecraft:iron_ingot",count:64}]}', failures)
    if scenario == "drills":
        for row in range(32):
            zz = z - 16 + row
            command(ctl_bin, sock, f"setblock {x-2} {y-6} {zz} create:creative_motor[facing=east]", failures)
            command(ctl_bin, sock, f"setblock {x-1} {y-6} {zz} create:mechanical_drill[facing=east]", failures)
            command(ctl_bin, sock, f"setblock {x} {y-6} {zz} minecraft:stone", failures)
    if scenario == "saws":
        for row in range(32):
            zz = z - 16 + row
            command(ctl_bin, sock, f"setblock {x} {y-6} {zz-0} create:mechanical_saw[facing=up,axis_along_first=false]",
                    failures) if row % 2 == 0 else None
            if row % 2 == 0:
                command(ctl_bin, sock, f"setblock {x} {y-6} {zz-1} create:creative_motor[facing=south]", failures)
    if scenario == "spouts":
        for row in range(10):
            zz, x0 = z - 15 + 3 * row, x
            command(ctl_bin, sock, f"fill {x0-7} {y-7} {zz-1} {x0+4} {y-7} {zz+2} stone", failures)
            command(ctl_bin, sock, f"fill {x0-6} {y-6} {zz-1} {x0-2} {y-6} {zz+2} glass", failures)
            command(ctl_bin, sock, f"fill {x0-5} {y-6} {zz} {x0-3} {y-6} {zz+1} water", failures)
            command(ctl_bin, sock, f"fill {x0-2} {y-6} {zz} {x0-1} {y-6} {zz} create:fluid_pipe", failures)
            command(ctl_bin, sock, f"setblock {x0} {y-6} {zz} create:mechanical_pump[facing=east]", failures)
            command(ctl_bin, sock, f"fill {x0+1} {y-6} {zz} {x0+2} {y-6} {zz} create:fluid_pipe", failures)
            command(ctl_bin, sock, f"setblock {x0+3} {y-6} {zz} create:spout", failures)
            command(ctl_bin, sock, f"setblock {x0+3} {y-7} {zz} air", failures)
            command(ctl_bin, sock, f"setblock {x0+3} {y-8} {zz} "
                    'create:depot{Items:[{Slot:0b,id:"minecraft:bucket",count:16}]}', failures)
            command(ctl_bin, sock, f"setblock {x0} {y-5} {zz} create:cogwheel[axis=x]", failures)
            command(ctl_bin, sock, f"setblock {x0+1} {y-5} {zz} create:creative_motor[facing=west]", failures)
    if scenario in ("funnels", "chests"):
        # A floor in front, so an extracted stack lies there and the funnel waits for it.
        command(ctl_bin, sock, f"fill {x-4} {y-7} {z-16} {x-1} {y-7} {z+15} stone", failures)
        items = ",".join(f'{{Slot:{i}b,id:"minecraft:cobblestone",count:64}}' for i in range(27))
        for row in range(32):
            zz = z - 16 + row
            command(ctl_bin, sock, f"setblock {x-4} {y-6} {zz} minecraft:chest{{Items:[{items}]}}", failures)
            command(ctl_bin, sock, f"setblock {x+4} {y-7} {zz} minecraft:chest", failures)
            if scenario == "chests":
                # The stack a funnel would have put in front, lying there.
                command(ctl_bin, sock, f"summon minecraft:item {x-2.25} {y-6} {zz+0.5} "
                        "{Item:{id:'minecraft:cobblestone',count:64},PickupDelay:32767}", failures)
                continue
            command(ctl_bin, sock, f"setblock {x-3} {y-6} {zz} create:andesite_funnel[facing=east,extracting=true]",
                    failures)
            command(ctl_bin, sock, f"setblock {x+4} {y-6} {zz} create:andesite_funnel[facing=up,extracting=false]",
                    failures)
    if scenario == "drains":
        for row in range(10):
            zz, x0 = z - 15 + 3 * row, x
            command(ctl_bin, sock, f"fill {x0-3} {y-7} {zz} {x0+4} {y-7} {zz} stone", failures)
            command(ctl_bin, sock, f"setblock {x0-2} {y-6} {zz} create:item_drain", failures)
            command(ctl_bin, sock, f"setblock {x0-1} {y-6} {zz} create:fluid_pipe", failures)
            command(ctl_bin, sock, f"setblock {x0} {y-6} {zz} create:mechanical_pump[facing=east]", failures)
            command(ctl_bin, sock, f"fill {x0+1} {y-6} {zz} {x0+2} {y-6} {zz} create:fluid_pipe", failures)
            command(ctl_bin, sock, f"setblock {x0+3} {y-6} {zz} create:fluid_tank", failures)
            command(ctl_bin, sock, f"setblock {x0} {y-5} {zz} create:cogwheel[axis=x]", failures)
            command(ctl_bin, sock, f"setblock {x0+1} {y-5} {zz} create:creative_motor[facing=west]", failures)
    if scenario in ("pistons", "chassis"):
        for row in range(16):
            zz = z - 16 + 2 * row
            command(ctl_bin, sock, f"fill {x-18} {y-6} {zz} {x-11} {y-6} {zz} create:piston_extension_pole[facing=east]",
                    failures)
            command(ctl_bin, sock, f"setblock {x-10} {y-6} {zz} "
                    "create:mechanical_piston[facing=east,axis_along_first=true,state=retracted]", failures)
            if scenario == "chassis":
                command(ctl_bin, sock, f"setblock {x-9} {y-6} {zz} "
                        "create:linear_chassis[axis=y,sticky_top=true]", failures)
                command(ctl_bin, sock, f"fill {x-9} {y-5} {zz} {x-9} {y-3} {zz} minecraft:stone", failures)
            else:
                command(ctl_bin, sock, f"setblock {x-9} {y-6} {zz} minecraft:stone", failures)
    if scenario == "burners":
        command(ctl_bin, sock, f"fill {x-16} {y-5} {z-16} {x+15} {y-5} {z+15} "
                "create:blaze_burner[blaze=smouldering,facing=south]", failures)
    if scenario == "crushers":
        for row in range(30):
            zz = z - 30 + 2 * row
            # Wheels at x-2 and x (z axis), driven -16 and +16 by motors on either side.
            command(ctl_bin, sock, f"setblock {x-2} {y-4} {zz} create:crushing_wheel[axis=z]", failures)
            command(ctl_bin, sock, f"setblock {x} {y-4} {zz} create:crushing_wheel[axis=z]", failures)
            command(ctl_bin, sock, f"setblock {x-2} {y-4} {zz+1} create:creative_motor[facing=north]", failures)
            command(ctl_bin, sock, f"setblock {x} {y-4} {zz-1} create:creative_motor[facing=south]", failures)
    if scenario == "fans":
        for row in range(32):
            zz = z - 32 + 2 * row
            command(ctl_bin, sock, f"setblock {x-17} {y-4} {zz} create:creative_motor[facing=east]", failures)
            command(ctl_bin, sock, f"setblock {x-16} {y-4} {zz} create:encased_fan[facing=east]", failures)
            # A water source in front, held by glass below and beside it and iron bars ahead (the
            # current passes bars, `create:fan_transparent`).
            command(ctl_bin, sock, f"fill {x-15} {y-4} {zz-1} {x-15} {y-4} {zz+1} minecraft:glass", failures)
            command(ctl_bin, sock, f"setblock {x-15} {y-5} {zz} minecraft:glass", failures)
            command(ctl_bin, sock, f"setblock {x-14} {y-4} {zz} minecraft:iron_bars", failures)
            command(ctl_bin, sock, f"setblock {x-15} {y-4} {zz} minecraft:water", failures)
            for i in range(3):
                command(ctl_bin, sock, f"setblock {x-13+i} {y-5} {zz} "
                        'create:depot{Items:[{Slot:0b,id:"minecraft:ice",count:64}]}', failures)
    if scenario in ("logistics", "tunnels", "belts9"):
        belt = "facing=south,slope=horizontal,casing=false,waterlogged=false"
        for row in range(32):
            xx = x - 31 + 2 * row
            command(ctl_bin, sock, f"setblock {xx-1} {y-6} {z-24} create:creative_motor[facing=east]", failures)
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z-24} create:belt[{belt},part=start]", failures)
            command(ctl_bin, sock, f"fill {xx} {y-6} {z-23} {xx} {y-6} {z+22} create:belt[{belt},part=middle]",
                    failures)
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z+23} create:belt[{belt},part=end]", failures)
            # A stone past the end: the stacks queue up instead of falling off.
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z+24} minecraft:stone", failures)
            if scenario == "tunnels":
                command(ctl_bin, sock, f"fill {xx} {y-5} {z-14} {xx} {y-5} {z+6} create:andesite_tunnel", failures)
        # Wait for the chains to form, then drop a stack on every other segment of the first 42.
        time.sleep(1)
        for row in range(32):
            xx = x - 31 + 2 * row
            # Under tunnels (from segment 10) a stack would land inside one: the first 9 only.
            for s in range(0, 9 if scenario in ("tunnels", "belts9") else 42, 2):
                ctl(ctl_bin, sock, "command",
                    f"summon minecraft:item {xx + 0.5} {y - 5} {z - 24 + s + 0.5} "
                    "{Item:{id:'minecraft:iron_ingot',count:8}}")
    if scenario in ("redstone", "repeaters", "diodes"):
        diode = {"redstone": "create:powered_toggle_latch[facing=west]",
                 "diodes": "create:pulse_extender[facing=west]"}.get(scenario, "minecraft:repeater[facing=west]")
        for row in range(32):
            zz = z - 31 + 2 * row
            command(ctl_bin, sock, f"fill {x-17} {y-7} {zz} {x+2} {y-7} {zz} minecraft:stone", failures)
            command(ctl_bin, sock, f"fill {x-14} {y-6} {zz} {x+1} {y-6} {zz} {diode}", failures)
            # Two observers watching each other: a clock whose pulses leave the east one's back.
            command(ctl_bin, sock, f"setblock {x-16} {y-6} {zz} minecraft:observer[facing=east]", failures)
            command(ctl_bin, sock, f"setblock {x-15} {y-6} {zz} minecraft:observer[facing=west]", failures)
    if scenario == "links":
        for row in range(32):
            zz = z - 31 + 2 * row
            command(ctl_bin, sock, f"fill {x-17} {y-7} {zz} {x+4} {y-7} {zz} minecraft:stone", failures)
            command(ctl_bin, sock, f"setblock {x-14} {y-6} {zz} create:redstone_link[facing=up]", failures)
            command(ctl_bin, sock, f"setblock {x+2} {y-6} {zz} create:redstone_link[facing=up,receiver=true]",
                    failures)
            command(ctl_bin, sock, f"setblock {x+3} {y-6} {zz} minecraft:redstone_lamp", failures)
            # Two observers watching each other: a clock whose pulses leave the east one's back.
            command(ctl_bin, sock, f"setblock {x-16} {y-6} {zz} minecraft:observer[facing=east]", failures)
            command(ctl_bin, sock, f"setblock {x-15} {y-6} {zz} minecraft:observer[facing=west]", failures)
    if scenario == "deployers":
        for i in range(16):
            xx = x - 15 + 2 * i
            command(ctl_bin, sock, f"setblock {xx} {y-7} {z} create:depot", failures)
            command(ctl_bin, sock, f"setblock {xx} {y-5} {z} create:deployer[facing=down,axis_along_first=false]",
                    failures)
            command(ctl_bin, sock, f"setblock {xx} {y-5} {z-1} create:creative_motor[facing=south]", failures)
        command(ctl_bin, sock, f"fill {x-16} {y-8} {z-2} {x+17} {y-8} {z+3} minecraft:stone", failures)
    if scenario in ("curves", "curvetracks"):
        command(ctl_bin, sock, f"fill {x-22} {y-7} {z-22} {x+22} {y-7} {z+22} minecraft:stone", failures)
        for gx in range(4):
            for gz in range(4):
                x0, z0 = x - 20 + 10 * gx, z - 20 + 10 * gz
                command(ctl_bin, sock, f"setblock {x0} {y-6} {z0} create:track[shape=xo]", failures)
                command(ctl_bin, sock, f"setblock {x0+8} {y-6} {z0+8} create:track[shape=zo]", failures)
    if scenario in ("deployeruse", "deployerstroke", "waterchurn", "waterstill", "honey"):
        # Wide enough that the water (7 blocks of flow) never pours off onto plants below: their
        # drops would be drawn as item entities.
        command(ctl_bin, sock, f"fill {x-12} {y-7} {z-34} {x+12} {y-7} {z+34} minecraft:stone", failures)
        if scenario == "waterchurn":
            for i in range(16):
                command(ctl_bin, sock, f"setblock {x-1} {y-6} {z - 24 + 3 * i} minecraft:dispenser[facing=east]"
                        "{Items:[{Slot:0b,id:'minecraft:water_bucket',count:1}]}", failures)
        if scenario in ("waterstill", "honey"):
            fluid = "minecraft:water" if scenario == "waterstill" else "create:honey"
            for i in range(16):
                command(ctl_bin, sock, f"setblock {x} {y-6} {z - 24 + 3 * i} {fluid}", failures)
        for i in range(16 if scenario in ("deployeruse", "deployerstroke") else 0):
            zz = z - 24 + 3 * i
            command(ctl_bin, sock, f"setblock {x-2} {y-6} {zz} create:deployer[facing=east,axis_along_first=false]",
                    failures)
            command(ctl_bin, sock, f"setblock {x-2} {y-6} {zz-1} create:creative_motor[facing=south]", failures)
    if scenario in ("gantries", "gantryshafts"):
        for row in range(16):
            zz = z - 16 + 2 * row
            command(ctl_bin, sock, f"fill {x-15} {y-7} {zz} {x+14} {y-7} {zz} create:gantry_shaft[facing=east]",
                    failures)
            if scenario == "gantryshafts":
                continue
            command(ctl_bin, sock, f"setblock {x-15} {y-6} {zz} create:gantry_carriage[facing=up]", failures)
            command(ctl_bin, sock, f"setblock {x-15} {y-5} {zz} minecraft:slime_block", failures)
            command(ctl_bin, sock, f"setblock {x-15} {y-4} {zz} minecraft:stone", failures)
    if scenario == "crafters":
        for i in range(16):
            xx = x - 16 + 2 * i
            command(ctl_bin, sock, f"setblock {xx} {y-5} {z} create:mechanical_crafter[facing=south,pointing=down]",
                    failures)
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z} create:mechanical_crafter[facing=south,pointing=down]",
                    failures)
            command(ctl_bin, sock, f"setblock {xx} {y-7} {z} minecraft:chest", failures)
            command(ctl_bin, sock, f"setblock {xx+1} {y-6} {z} create:cogwheel[axis=z]", failures)
            command(ctl_bin, sock, f"setblock {xx} {y-4} {z} minecraft:hopper[facing=down]"
                    "{Items:[{Slot:0b,id:'minecraft:coal',count:64}]}", failures)
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z+1} minecraft:hopper[facing=north]"
                    "{Items:[{Slot:0b,id:'minecraft:stick',count:64}]}", failures)
        command(ctl_bin, sock, f"setblock {x-17} {y-6} {z} create:cogwheel[axis=z]", failures)
        command(ctl_bin, sock, f"setblock {x-17} {y-6} {z-1} create:creative_motor[facing=south]", failures)
    if scenario in ("brass", "brassbelts"):
        belt = "facing=south,slope=horizontal,casing=false,waterlogged=false"
        for row in range(16):
            xx = x - 30 + 4 * row
            command(ctl_bin, sock, f"setblock {xx-1} {y-6} {z-15} create:creative_motor[facing=east]", failures)
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z-15} create:belt[{belt},part=start]", failures)
            command(ctl_bin, sock, f"fill {xx} {y-6} {z-14} {xx} {y-6} {z+13} create:belt[{belt},part=middle]",
                    failures)
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z+14} create:belt[{belt},part=end]", failures)
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z+15} minecraft:stone", failures)
            command(ctl_bin, sock, f"setblock {xx-1} {y-6} {z} create:depot", failures)
            command(ctl_bin, sock, f"setblock {xx+1} {y-6} {z} create:depot", failures)
            if scenario == "brass":
                command(ctl_bin, sock, f"setblock {xx} {y-5} {z} create:brass_tunnel", failures)
        time.sleep(1)
        for row in range(16):
            xx = x - 30 + 4 * row
            for seg in range(0, 9, 2):
                ctl(ctl_bin, sock, "command", f"summon minecraft:item {xx + 0.5} {y - 5} {z - 15 + seg + 0.5} "
                    "{Item:{id:'minecraft:iron_ingot',count:8}}")
    if scenario == "bearings":
        for i in range(16):
            bx, bz = x - 15 + 10 * (i % 4), z - 15 + 10 * (i // 4)
            command(ctl_bin, sock, f"setblock {bx} {y-6} {bz} create:mechanical_bearing[facing=up]", failures)
            command(ctl_bin, sock, f"setblock {bx} {y-5} {bz} minecraft:slime_block", failures)
            for dx, dz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                command(ctl_bin, sock, f"setblock {bx+dx} {y-5} {bz+dz} minecraft:stone", failures)
            command(ctl_bin, sock, f"setblock {bx} {y-7} {bz} create:creative_motor[facing=up]", failures)
    if scenario == "contraptions":
        for i in range(16):
            bx, bz = x - 15 + 10 * (i % 4), z - 15 + 10 * (i // 4)
            # A bearing facing up, its sails in a cross in the plane above it.
            command(ctl_bin, sock, f"setblock {bx} {y-6} {bz} create:windmill_bearing[facing=up]", failures)
            command(ctl_bin, sock, f"fill {bx-4} {y-5} {bz} {bx+4} {y-5} {bz} create:white_sail[facing=up]",
                    failures)
            command(ctl_bin, sock, f"fill {bx} {y-5} {bz-4} {bx} {y-5} {bz+4} create:white_sail[facing=up]",
                    failures)
            # Somewhere to stand within reach of its east side, the eyes below the sails.
            command(ctl_bin, sock, f"setblock {bx+2} {y-8} {bz} minecraft:glass", failures)
    return failures


def assemble_windmills(ctl_bin, sock, center):
    """Clicks each bearing with an empty hand until it turns (a click the harness loses is tried
    again); returns how many turn."""
    x, y, z = center
    turning = 0
    for i in range(16):
        bx, bz = x - 15 + 10 * (i % 4), z - 15 + 10 * (i // 4)
        for _ in range(4):
            ctl(ctl_bin, sock, "tp", str(bx + 2), str(y - 7), str(bz))
            try:
                ctl(ctl_bin, sock, "look-at-block", str(bx), str(y - 6), str(bz), "east")
                ctl(ctl_bin, sock, "interact", str(bx), str(y - 6), str(bz), "east")
            except RuntimeError as e:
                print(f"windmill {i}: {e}")
            time.sleep(1)
            ctl(ctl_bin, sock, "chat", f"/create_debug speed {bx} {y-6} {bz}")
            time.sleep(0.4)
            said = ctl(ctl_bin, sock, "messages", "1").get("chat", [""])[0]
            if said.endswith(": 2 rpm"):
                turning += 1
                break
    return turning


def arm_deployers(ctl_bin, sock, center):
    """Gives each deployer 64 oak planks with a click on its pole (`DeployerBlock.useItemOn`),
    then drops 64 shafts on its depot; returns how many took the planks."""
    x, y, z = center
    armed = 0
    for i in range(16):
        xx = x - 15 + 2 * i
        for _ in range(3):
            ctl(ctl_bin, sock, "command", "item replace entity @p hotbar.0 with minecraft:oak_planks 64")
            ctl(ctl_bin, sock, "select", "1")
            ctl(ctl_bin, sock, "tp", str(xx + 0.5), str(y - 7), str(z + 2.5))
            time.sleep(0.3)
            ctl(ctl_bin, sock, "look-at", str(xx + 0.5), str(y - 5 + 0.01), str(z + 0.5))
            time.sleep(0.3)
            ctl(ctl_bin, sock, "use-item")
            time.sleep(0.4)
            inv = ctl(ctl_bin, sock, "inventory")
            if not any(s.get("item") for s in inv.get("hotbar", [])):
                armed += 1
                break
        ctl(ctl_bin, sock, "command", f"summon minecraft:item {xx + 0.5} {y - 6} {z + 0.5} "
            "{Item:{id:'create:shaft',count:64}}")
    return armed


def arm_deployers_east(ctl_bin, sock, center, item):
    """Gives each east-facing deployer of `deployeruse` one `item` with a click on its pole; returns
    how many took it."""
    x, y, z = center
    armed = 0
    for i in range(16):
        zz = z - 24 + 3 * i
        for _ in range(3):
            ctl(ctl_bin, sock, "command", f"item replace entity @p hotbar.0 with {item} 1")
            ctl(ctl_bin, sock, "select", "1")
            ctl(ctl_bin, sock, "tp", str(x - 0.5), str(y - 6), str(zz + 1.7))
            time.sleep(0.3)
            ctl(ctl_bin, sock, "look-at", str(x - 0.999), str(y - 5.5), str(zz + 0.5))
            time.sleep(0.3)
            ctl(ctl_bin, sock, "use-item")
            time.sleep(0.4)
            inv = ctl(ctl_bin, sock, "inventory")
            if not any(s.get("item") for s in inv.get("hotbar", [])):
                armed += 1
                break
    return armed


# `waterchurn`'s clock: a redstone block put on each dispenser and taken off again, staggered, so
# each fires once every 4 s.
WATER_CHURN = """
import subprocess, sys, time
ctl_bin, sock, x, y, z, seconds = sys.argv[1], sys.argv[2], *map(int, sys.argv[3:6]), float(sys.argv[6])
end, k = time.time() + seconds, 0
while time.time() < end:
    i = k % 16
    block = "minecraft:redstone_block" if (k // 16) % 2 == 0 else "minecraft:air"
    subprocess.run([ctl_bin, "--socket", sock, "command", f"setblock {x - 1} {y + 1} {z - 24 + 3 * i} {block}"],
                   capture_output=True)
    k += 1
    time.sleep(2 / 16)
"""


def arm_curves(ctl_bin, sock, center):
    """Joins each pair of `curves` with the track item, as a player clicks (first the east-west
    track looking east, then the north-south one looking south); returns how many were joined."""
    x, y, z = center
    ctl(ctl_bin, sock, "command", "item replace entity @p hotbar.0 with create:track 64")
    ctl(ctl_bin, sock, "select", "1")
    for gx in range(4):
        for gz in range(4):
            x0, z0 = x - 20 + 10 * gx, z - 20 + 10 * gz
            for (px, pz, lx, lz) in ((x0 - 1.5, z0 + 0.5, x0 + 0.5, z0 + 0.5),
                                     (x0 + 8.5, z0 + 6.5, x0 + 8.5, z0 + 8.5)):
                ctl(ctl_bin, sock, "tp", str(px), str(y - 6), str(pz))
                time.sleep(0.3)
                ctl(ctl_bin, sock, "look-at", str(lx), str(y - 6 + 0.06), str(lz))
                time.sleep(0.2)
                ctl(ctl_bin, sock, "use-item")
                time.sleep(0.3)
    joined = 0
    for gx in range(4):
        for gz in range(4):
            x0, z0 = x - 20 + 10 * gx, z - 20 + 10 * gz
            b = ctl(ctl_bin, sock, "blocks", str(x0), str(y - 6), str(z0), str(x0), str(y - 6), str(z0))
            joined += sum(1 for e in b.get("blocks", []) if "turn=true" in e[3])
    return joined


def count_create(ctl_bin, sock, center):
    """Create blocks the scene really holds, counted in 16-row bands: one `blocks` reply lists at
    most 4,096 blocks."""
    x, y, z = center
    total = 0
    for band in range(z - 32, z + 33, 16):
        found = ctl(ctl_bin, sock, "blocks", str(x - 32), str(y - 7), str(band), str(x + 32), str(y - 4),
                    str(min(band + 15, z + 32)))
        # Crushing wheel controllers come and go with the wheels' speed, and fluids spread: not
        # counted.
        total += sum(n for k, n in found.get("counts", {}).items()
                     if k.startswith("create:") and not k.startswith(("create:crushing_wheel_controller",
                                                                      "create:honey", "create:chocolate")))
    return total


def run_once(args, scenario):
    work = Path(tempfile.mkdtemp(prefix="create-bench-"))
    # As perf_bench.py: no frame cap and no VSync, so frame times show the work, not the cap; and
    # no idle cap (vanilla's `inactivityFpsLimit` "afk" drops to 30 fps a minute after the last
    # input, which a long scene build reaches).
    (work / "options.txt").write_text("maxFps:260\nenableVsync:false\nrenderDistance:%d\n"
                                      "simulationDistance:12\npauseOnLostFocus:false\n"
                                      "inactivityFpsLimit:minimized\n" % args.distance)
    game = Path(args.game).resolve()
    launch = ["--world", "B", "--creative", "--seed", "1234", "--no-pause", "--weather", "clear",
              "--time", "6000", "--render-distance", str(args.distance)]
    if scenario != "off":
        launch += ["--dev-mod", str(Path(args.content).resolve())]
    out = subprocess.run([str(HEADLESS), str(game), *launch], cwd=work, capture_output=True, text=True)
    sock = re.search(r"SOCK=(\S+)", out.stdout).group(1)
    scratch = Path(sock).parent
    ctl_bin = game.parent / "horizon-ctl"
    try:
        launch_state = json.loads((scratch / "launch.log").read_text().strip().splitlines()[-1])
        gpu = launch_state["renderer"]["gpu"]
        if "llvmpipe" in gpu.lower() or "BMG" not in gpu:
            raise SystemExit(f"wrong renderer: {gpu}")
        ctl(ctl_bin, sock, "wait-ready")
        spawn = ctl(ctl_bin, sock, "state")["player"]["position"]
        center = (int(spawn[0]), int(spawn[1]) + LIFT, int(spawn[2]))
        expected = {"off": 0, "idle": 0, "static": 1024, "kinetic": 4096, "chains": 0,
                    "processing": 528, "deployers": 48, "deployeruse": 32, "deployerstroke": 32, "waterchurn": 0, "waterstill": 0, "honey": 0, "curves": 32, "curvetracks": 32, "gauges": 64 * 64, "bearings": 32, "brass": 16 * 34, "brassbelts": 16 * 33, "chassis": 16 * 10, "crafters": 16 * 3 + 2, "gantries": 16 * 31, "gantryshafts": 16 * 30, "logistics": 32 * 48 + 32, "tunnels": 32 * 48 + 32 + 32 * 21, "belts9": 32 * 48 + 32, "redstone": 512, "diodes": 512, "links": 64,
                    "repeaters": 0, "contraptions": 16 * 18, "fans": 32 * 5, "crushers": 30 * 4, "burners": 1024, "mixers": 32 * 4, "pumps": 10 * 8, "trains": 12 * 60, "signals": 12 * 62, "pistons": 16 * 9, "goggles": 4096, "presses": 4096, "chutes": 16 * 3, "drills": 64, "saws": 32, "spouts": 10 * 8, "funnels": 64, "chests": 0, "drains": 10 * 8, "boilers": 10 * 17, "tanks": 10 * 16}[scenario]
        for attempt in range(5):
            failures = build(ctl_bin, sock, scenario, center)
            # Commands land at the server's next tick: a one-command scene is counted after it.
            time.sleep(1)
            have = count_create(ctl_bin, sock, center)
            if have == expected:
                break
            print(f"attempt {attempt}: {have} of {expected} Create blocks; failed: {failures[:3]}")
            time.sleep(2)
        else:
            raise SystemExit(f"scene holds {have} Create blocks, not {expected}")
        x, y, z = center
        turning = None
        if scenario == "crushers":
            # The controllers appear once the wheels turn; then a stack falls into each gap.
            time.sleep(2)
            for row in range(30):
                ctl(ctl_bin, sock, "command", f"summon minecraft:item {x-0.5} {y-2} {z-30+2*row+0.5} "
                    "{Item:{id:'minecraft:gravel',count:16},PickupDelay:32767}")
        if scenario in ("trains", "signals"):
            for row in range(12):
                ctl(ctl_bin, sock, "chat", f"/create_debug assemble {x-27} {y-6} {z-24+4*row-1}")
        if scenario == "deployers":
            armed = arm_deployers(ctl_bin, sock, center)
            if armed != 16:
                raise SystemExit(f"only {armed} of 16 deployers took their planks")
        if scenario == "curves":
            joined = arm_curves(ctl_bin, sock, center)
            if joined != 16:
                raise SystemExit(f"only {joined} of 16 curves were joined")
        if scenario in ("deployeruse", "deployerstroke"):
            item = "minecraft:water_bucket" if scenario == "deployeruse" else "minecraft:stick"
            armed = arm_deployers_east(ctl_bin, sock, center, item)
            if armed != 16:
                raise SystemExit(f"only {armed} of 16 deployers took their {item}")
        if scenario == "contraptions":
            turning = assemble_windmills(ctl_bin, sock, center)
            if turning != 16:
                raise SystemExit(f"only {turning} of 16 windmills turn")
        if args.no_mobs:
            # Mobs wandering into view swing the frame time (vanilla's entity drawing) from run to run.
            ctl(ctl_bin, sock, "command", "gamerule spawn_mobs false")
            ctl(ctl_bin, sock, "command", "kill @e[type=!player,type=!item]")
        ctl(ctl_bin, sock, "tp", str(x), str(y), str(z))
        ctl(ctl_bin, sock, "fly", str(x), str(y), str(z), str(x), str(y), str(z), "0.1", "40")
        ctl(ctl_bin, sock, "wait-ready")
        time.sleep(args.settle)
        if scenario == "drains":
            for row in range(10):
                ctl(ctl_bin, sock, "command", f"summon minecraft:item {x-1.5} {y-4} {z-15+3*row+0.5} "
                    "{Item:{id:'minecraft:water_bucket',count:1},PickupDelay:32767}")
        if scenario == "saws":
            for row in range(0, 32, 2):
                ctl(ctl_bin, sock, "command", f"summon minecraft:item {x+0.5} {y-5} {z-16+row+0.5} "
                    "{Item:{id:'create:andesite_alloy',count:64},PickupDelay:32767}")
        if scenario in ("gantries", "gantryshafts"):
            for row in range(16):
                ctl(ctl_bin, sock, "command", f"setblock {x+15} {y-7} {z-16+2*row} create:creative_motor[facing=west]")
        if scenario in ("pistons", "chassis"):
            for row in range(16):
                ctl(ctl_bin, sock, "command", f"setblock {x-10} {y-5} {z-16+2*row} create:creative_motor[facing=down]")
        if scenario == "goggles":
            ctl(ctl_bin, sock, "command", "item replace entity @p armor.head with create:goggles")
        if scenario in ("trains", "signals"):
            turning = [ctl(ctl_bin, sock, "chat", f"/create_debug train {x+27} {y-6} {z-24+4*row-1}")
                       for row in range(12)]
            time.sleep(0.5)
            turning = ctl(ctl_bin, sock, "messages", "12").get("chat")
        # A slow circle-ish line over the scenario, looking down at it.
        churn = None
        if scenario == "waterchurn":
            churn = subprocess.Popen(["python3", "-c", WATER_CHURN, ctl_bin, sock, str(x), str(y - 6), str(z),
                                      str(args.seconds + 4)])
        summary = ctl(ctl_bin, sock, "fly", str(x - 12), str(y), str(z - 12), str(x + 12), str(y), str(z + 12),
                      str(args.seconds), "40", "raw", timeout=int(args.seconds) + 120)
        if churn:
            churn.wait()
        if args.screenshot:
            ctl(ctl_bin, sock, "screenshot", str(Path(args.screenshot).resolve()))
        pressed = turning
        if scenario in ("logistics", "tunnels", "belts9"):
            # The stacks are on the belts, not lying around as entities.
            ents = ctl(ctl_bin, sock, "entities")
            pressed = sum(1 for e in ents.get("entities", []) if "item" in str(e.get("type", "")))
        if scenario in ("redstone", "repeaters"):
            # The counters really count: the states along one chain.
            chain = ctl(ctl_bin, sock, "blocks", str(x - 14), str(y - 6), str(z - 31), str(x + 1), str(y - 6),
                        str(z - 31)).get("blocks", [])
            # A latch's output is `powering`; a repeater's, `powered`. Two reads a second apart:
            # a counter changes between them.
            key = "powering=true" if scenario == "redstone" else "powered=true"
            first = "".join("1" if key in b[3] else "0" for b in sorted(chain))
            time.sleep(1)
            chain = ctl(ctl_bin, sock, "blocks", str(x - 14), str(y - 6), str(z - 31), str(x + 1), str(y - 6),
                        str(z - 31)).get("blocks", [])
            pressed = [first, "".join("1" if key in b[3] else "0" for b in sorted(chain))]
        if scenario == "gantries":
            # The carriages are moving: none left where they started.
            pressed = ctl(ctl_bin, sock, "blocks", str(x - 15), str(y - 6), str(z - 16), str(x - 15), str(y - 4),
                          str(z + 14)).get("counts", {})
        if scenario in ("brass", "brassbelts"):
            pressed = ctl(ctl_bin, sock, "command", f"data get block {x-31} {y-6} {z} Items").get("chat")
        if scenario == "bearings":
            # The structures are out of the world, turning: no slime block left in place.
            found = ctl(ctl_bin, sock, "blocks", str(x - 16), str(y - 5), str(z - 16), str(x + 16), str(y - 5),
                        str(z + 16)).get("counts", {})
            pressed = found
        if scenario == "crafters":
            pressed = ctl(ctl_bin, sock, "command", f"data get block {x-16} {y-7} {z} Items").get("chat")
        if scenario == "deployers":
            pressed = ctl(ctl_bin, sock, "command", f"data get block {x-15} {y-7} {z} Items").get("chat")
        if scenario == "honey":
            found = ctl(ctl_bin, sock, "blocks", str(x), str(y - 6), str(z - 24), str(x), str(y - 6),
                        str(z + 21)).get("blocks", [])
            pressed = sum(1 for b in found if b[3].startswith("create:honey"))
        if scenario in ("deployeruse", "waterstill", "waterchurn"):
            # Water where the hands reach, in some of the 16 at any moment.
            found = ctl(ctl_bin, sock, "blocks", str(x), str(y - 6), str(z - 24), str(x), str(y - 6),
                        str(z + 21)).get("blocks", [])
            pressed = sum(1 for b in found if b[3].startswith("minecraft:water"))
        if scenario in ("boilers", "tanks"):
            # Each row's shaft (powered by its engine) and the speed of the first one.
            found = ctl(ctl_bin, sock, "blocks", str(x + 4), str(y - 6), str(z - 26), str(x + 4), str(y - 6),
                        str(z + 28)).get("blocks", [])
            ctl(ctl_bin, sock, "chat", f"/create_debug speed {x+4} {y-6} {z-26}")
            time.sleep(0.5)
            pressed = [sum(1 for b in found if b[3].startswith("create:powered_shaft")),
                       ctl(ctl_bin, sock, "messages", "1").get("chat")]
        if scenario == "links":
            # The receivers follow the clocks: the lamps read twice, a moment apart.
            reads = []
            for _ in range(4):
                lamps = ctl(ctl_bin, sock, "blocks", str(x + 3), str(y - 6), str(z - 31), str(x + 3),
                            str(y - 6), str(z + 31)).get("blocks", [])
                reads.append(sum(1 for b in lamps if "lit=true" in b[3]))
                time.sleep(0.15)
            pressed = reads
        if scenario == "processing":
            # The work really ran: sheets in one depot's outputs.
            pressed = ctl(ctl_bin, sock, "command", f"data get block {x-8} {y-6} {z}").get("chat")
        if scenario == "spouts":
            pressed = ctl(ctl_bin, sock, "command", f"data get block {x+3} {y-8} {z-15}").get("chat")
        if scenario == "chutes":
            pressed = ctl(ctl_bin, sock, "command", f"data get block {x-12} {y-7} {z-12} Items").get("chat")
        if scenario == "mixers":
            # Mixed: alloy in one basin's outputs.
            pressed = ctl(ctl_bin, sock, "command", f"data get block {x-16} {y-6} {z-16}").get("chat")
        if scenario == "crushers":
            # Crushed: sand lying below the gaps.
            ents = ctl(ctl_bin, sock, "entities")
            pressed = sorted({str(e.get("item", {}).get("item")) for e in ents.get("entities", [])
                              if "item" in str(e.get("type", ""))})
        if scenario == "fans":
            # The current really washed: one row's depots.
            pressed = [ctl(ctl_bin, sock, "command", f"data get block {x-13+i} {y-5} {z}").get("chat")
                       for i in range(3)]
        frames = summary.get("frame_ms") or []
        return {"gpu": gpu, "create_blocks": have, "frames": len(frames),
                "frame_ms_p50": statistics.median(frames) if frames else None,
                "frame_ms_p99": sorted(frames)[int(len(frames) * 0.99) - 1] if frames else None,
                "fps": len(frames) / args.seconds,
                "tick_ms": summary.get("tick_ms"), "tick_ms_max": summary.get("tick_ms_max"),
                # Where a frame's time goes: the game's CPU work and the GPU's (medians).
                "cpu_ms_p50": statistics.median(summary["cpu_ms"]) if summary.get("cpu_ms") else None,
                "gpu_ms_p50": statistics.median(summary["gpu_ms"]) if summary.get("gpu_ms") else None,
                "check": pressed}
    finally:
        try:
            ctl(ctl_bin, sock, "quit", timeout=60)
        except Exception:
            pass
        pid = scratch / "xvfb.pid"
        if pid.exists():
            subprocess.run(["kill", pid.read_text().strip()], capture_output=True)
        if args.keep_logs:
            keep = Path(args.keep_logs) / f"{scenario}-{int(time.time())}"
            keep.mkdir(parents=True, exist_ok=True)
            for log in scratch.glob("*.log"):
                shutil.copy(log, keep / log.name)
        shutil.rmtree(scratch, ignore_errors=True)
        shutil.rmtree(work, ignore_errors=True)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--game", default=str(ROOT / "target/horizon-cargo/fast/horizon-client"))
    p.add_argument("--content", default=str(ROOT / "target/create-content"))
    p.add_argument("--scenario", choices=["off", "idle", "static", "kinetic", "chains", "processing",
                                          "logistics", "tunnels", "belts9", "deployers", "deployeruse", "deployerstroke", "waterchurn", "waterstill", "honey", "curves", "curvetracks", "gauges", "bearings", "signals", "brass", "brassbelts", "chassis", "crafters", "gantries", "gantryshafts", "redstone", "repeaters", "diodes", "links", "contraptions", "fans", "crushers", "burners", "mixers", "pumps", "trains", "pistons", "goggles", "presses", "chutes", "drills", "saws", "spouts", "funnels", "chests", "drains", "boilers", "tanks"],
                   required=True)
    p.add_argument("--repeats", type=int, default=3)
    p.add_argument("--seconds", type=float, default=12.0)
    p.add_argument("--settle", type=float, default=3.0)
    p.add_argument("--distance", type=int, default=12)
    p.add_argument("--out")
    p.add_argument("--screenshot", help="a screenshot of the scene after the last flight")
    p.add_argument("--no-mobs", action="store_true",
                   help="turn mob spawning off and clear mobs before measuring (a steadier frame time)")
    p.add_argument("--keep-logs", help="copy each run's game logs into this folder")
    args = p.parse_args()
    runs = [run_once(args, args.scenario) for _ in range(args.repeats)]
    keys = ["fps", "frame_ms_p50", "frame_ms_p99", "tick_ms", "tick_ms_max"]
    result = {"scenario": args.scenario, "distance": args.distance, "seconds": args.seconds,
              "loadavg_before": os.getloadavg(), "runs": runs,
              "mean": {k: statistics.mean(r[k] for r in runs) for k in keys},
              "stdev": {k: statistics.stdev([r[k] for r in runs]) if len(runs) > 1 else 0.0 for k in keys}}
    text = json.dumps(result, indent=2)
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(text + "\n")
    print(text)


if __name__ == "__main__":
    main()
