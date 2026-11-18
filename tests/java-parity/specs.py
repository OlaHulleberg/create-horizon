"""Input matrices ported from Delete228/create-horizon, independent of either implementation.

The Java method bodies are fetched from the pinned MIT source by the runner.
Each template supplies explicit test contexts, not a Minecraft/NeoForge runtime.
"""
from dataclasses import dataclass
import itertools
import math
import random
from matrices.basin import cases as basin_cases
from matrices.transport import cases as transport_cases
from matrices.transport_processing import cases as transport_processing_cases
from matrices.fan_entity import cases as fan_entity_cases


@dataclass(frozen=True)
class Spec:
    name: str
    template: str
    methods: tuple
    cases: object
    scope: str
    numeric_tags: str = ""
    tolerance: float = 0.0
    projection: object = None
    observations_equal: object = None


KINETICS = "content/kinetics/"


def method(marker, path, name, selector=None):
    return marker, "src/main/java/com/simibubi/create/" + path + ".java", name, selector


def processing_cases():
    return ([f"D {count} {bulk}" for count, bulk in itertools.product(range(1, 65), range(2))]
            + [f"S {rpm / 4}" for rpm in range(-4096, 4097)]
            + [f"H {tick / 4}" for tick in range(961)])


def placement_cases():
    return [f"{kind} {mask} {look} {face} {shift} {against}"
            for kind, mask, look, face, shift in itertools.product(
                range(3), range(64), range(6), range(6), range(2))
            for against in (range(4) if kind == 2 else [0])]


def rotation_cases():
    nodes = [(kind, axis, face, power)
             for kind, axis in itertools.product(range(8), range(3))
             for face in (range(6) if kind == 3 else [0])
             for power in ([0, 1] if kind == 6 else [0])]
    offsets = [p for p in itertools.product(range(-1, 2), repeat=3) if 1 <= sum(map(abs, p)) <= 2]
    return [" ".join(map(str, (*a, *b, *p))) for a, b, p in itertools.product(nodes, nodes, offsets)]


def milling_cases():
    cases = [f"S {rpm / 8}" for rpm in range(-8193, 8194)]
    cases += [f"S {rpm}" for rpm in [-1e30, -8193, -8192, 8192, 8193, 1e30]]
    cases += ["T " + " ".join(map(str, values)) for values in itertools.product(
        [0, .5, -16, 64, 256, 8192], [0, 1, -15, 100], [0, 1, 4], [0, 1],
        [0, 1, 200], [0, 63, 64, 98, 99], [1, 12, 205], [-1, 1])]
    cases += [f"R {seed} {count} {chance}" for seed, count, chance in itertools.product(
        [0, 1, 17, 123456789, 281474976710655], [0, 1, 2, 9, 64, 99],
        [0, .01, .25, .5, .75, .99, 1, 2])]
    return cases


def drilling_cases():
    cases = [f"{kind} {rpm / 8}" for kind, rpm in itertools.product(["S", "D"], range(-8192, 8193))]
    cases += ["T " + " ".join(map(str, values)) for values in itertools.product(
        [0, .25, -16, 32, 256], [-1, 0, .5, 1.5, 50], [0, 1, 2, 4], [0, 5, 9],
        [-1, 0, 1, 15], [1, 12, 60], [-1, 3], [5], [22])]
    return cases


def visual_cases():
    cases = ["O " + " ".join(map(str, p)) for p in itertools.product(
        range(3), range(-3, 4), range(-3, 4), range(-3, 4), range(2))]
    cases += [f"A {t} {r} {o}" for t, r, o in itertools.product(
        [0, .25, 1, 20, 1200, 24000, 1000000, 16777216], [-256, -32, 0, 16, 256], [0, 11.25, 22.5])]
    cases += [f"S {r / 8}" for r in range(-2048, 2049)]
    cases += [f"C {r} {p / 8}" for r, p in itertools.product([-256, -64, -32, 0, 32, 64, 256], range(9))]
    cases += [f"W {score} {size}" for score, size in itertools.product(range(-12, 13), [1, 2])]
    return cases


def fan_cases():
    cases = [f"{tag} {rpm / 8}" for tag in "MV" for rpm in range(-4096, 4097)]
    cases += [f"P {rpm} {direction}" for rpm, direction in itertools.product(
        [-256, -16, -.125, 0, .125, 16, 256], range(6))]
    sizes = [1, 2, 15, 16, 17, 31, 32, 33, 47, 48, 49, 63, 64]
    for mode, kind, count, valid in itertools.product("DT", range(4), sizes, range(2)):
        cases.append(f"{mode} -1 0 {kind},{count},{valid},{150 * ((count - 1) // 16 + 1) + 5}")
    for mode, a, b, count in itertools.product("DT", range(4), range(4), sizes):
        other = (a + 1) % 4
        cases.append(f"{mode} -1 0 {a},{count},1,3 {a},64,0,3 "
                     f"{b},1,0,3 {other},17,1,302 {a},{count},1,4")
    for mode, kind, time, valid in itertools.product("DT", range(4), [-3, -2, -1, 0, 1, 2, 150, 151], range(2)):
        cases.append(f"{mode} {kind} {time} {kind},17,{valid},4")
    rng = random.Random(0xCFE05)
    for mode in "DT":
        for _ in range(250):
            phases = " ".join(f"{rng.randrange(4)},{rng.choice(sizes)},{rng.randrange(2)},{rng.randrange(1, 180)}"
                              for _ in range(8))
            cases.append(f"{mode} -1 0 {phases}")
    return cases


def burner_cases():
    times = [-2147483648, -1, 0, 1, 2, 124, 125, 126, 500, 501, 9999, 10000, 2147483647]
    cases = ["R " + " ".join(map(str, v)) for v in itertools.product(
        [-1, 0, 7, 3200], [-1, 0, 8, 1600], [0, 1], [-1, 0, 1, 9, 1600, 2147483647], [0, 1])]
    cases += ["T " + " ".join(map(str, v)) for v in itertools.product(
        range(3), times, [0, 1], range(5), [1, 2, 126, 5001])]
    cases += ["C " + " ".join(map(str, v)) for v in itertools.product(
        range(3), [-1, 0, 1, 500, 10000], [0, 1], range(5))]
    cases += ["I " + " ".join(map(str, v)) for v in itertools.product(
        range(3), times, [0, 1], [1, 3, 4], [1, 2], [1, 1600, 2147483647], [0, 1], [0, 1])]
    rng = random.Random(610)
    for _ in range(10000):
        candidate = rng.randrange(3)
        cases.append("I " + " ".join(map(str, [rng.randrange(3), rng.randint(-2147483648, 2147483647),
            rng.randrange(2), rng.randrange(5), candidate,
            rng.randint(-2147483648, 2147483647) if candidate else 0, rng.randrange(2), rng.randrange(2)])))
    return cases


def burner_state_projection(case, observation):
    # Public Burner state only. World heat updates and notification counts are
    # intentionally not asserted through a pure-engine adapter.
    return observation if case.startswith("R ") else " ".join(observation.split()[:4])


def basin_state_projection(case, observation):
    if not case.startswith("A "):
        return observation
    # Engine apply exposes success and inventory, not host RNG draw/notification counters.
    fields = observation.split()
    return " ".join([fields[0], *fields[3:]])


def milling_state_projection(case, observation):
    # Concrete PRNGs differ deliberately. Compare their observable output counts;
    # classify seeded differences rather than emulating Java's generator in Rust.
    return observation.split()[0] if case.startswith("R ") else observation


def fan_motion_projection(case, observation):
    # Only the motion vector is in the public fan push contract. Entity selection,
    # media effects and fall/flight counters belong to SDK glue.
    return " ".join(observation.split()[1:4]) if case.startswith("M ") else observation


def belt_surfaces_equal(java, rust):
    left, right = java.split(), rust.split()
    if len(left) % 3 or len(right) % 3:
        raise ValueError("Belt surface observations must be model/shift/scroll triplets")
    if len(left) != len(right):
        return False
    for i in range(0, len(left), 3):
        if left[i:i+2] != right[i:i+2]:
            return False
        a, b = float(left[i+2]), float(right[i+2])
        if not math.isfinite(a) or not math.isfinite(b) or abs(a-b) > 2e-6:
            return False
    return bool(left)


SPECS = (
    Spec("motion-primitives", "MotionOracle", (
        method("WINDMILL", "content/contraptions/bearing/WindmillBearingBlockEntity", "getGeneratedSpeed"),
        method("DIRECTION", "content/contraptions/bearing/WindmillBearingBlockEntity", "getAngleSpeedDirection"),
        method("RANGE", "content/contraptions/pulley/PulleyBlockEntity", "getExtensionRange"),
        method("MOVEMENT", "content/contraptions/piston/LinearActuatorBlockEntity", "getMovementSpeed"),
        method("PISTON", "content/contraptions/piston/MechanicalPistonBlockEntity", "getMovementSpeed"),
        method("LINEAR", KINETICS + "base/KineticBlockEntity", "convertToLinear"),
        method("GANTRY", KINETICS + "gantry/GantryShaftBlockEntity", "getPinionMovementSpeed"),
    ), lambda: ([f"W {sails} {direction}" for sails, direction in itertools.product(range(257), range(2))]
                + [f"R {y} {minimum}" for y, minimum in itertools.product(range(-65, 501), [-64, 0, 64])]
                + [f"{op} {rpm / 8}" for op, rpm in itertools.product("LG", range(-8192, 8193))]
                + [f"P {rpm} {face} {offset} 4" for rpm, face, offset in itertools.product(
                    [-1024, -256, -32, 0, 32, 256, 1024], range(6), [0, .125, .5, 3.75, 4])]),
         "Additional primitive checks derived from the migrated bearing/pulley/gantry reference work: active windmill speed, rope range and server translation speeds; no assembly, sequence limit, client offsets or entity collisions.", "WRLPG", 1e-6),
    Spec("belt-surface", "BeltSurfaceOracle", (
        method("PARTIAL", KINETICS + "belt/BeltRenderer", "getBeltPartial"),
        method("RENDER", KINETICS + "belt/BeltRenderer", "renderSafe"),
    ), lambda: [" ".join(map(str, (*row, clock, partial)))
                for row in itertools.product(range(4), range(5), range(4), [-256, -32, 0, 32, 256])
                for clock, partial in [(0, 0), (10, .125), (10000, .875), (120000, .25)]],
         "Fallback renderer surface choice and UV phase only (normalized sprite height); poses, instancing, shaders, motor UI and pixels excluded.",
         observations_equal=belt_surfaces_equal),
    Spec("fan-entity", "FanEntityOracle", tuple(
        method(marker, KINETICS + path, name, "class:" + owner)
        for marker, path, owner, name in [
            ("BLAST", "fan/processing/AllFanProcessingTypes", "BlastingType", "affectEntity"),
            ("HAUNT", "fan/processing/AllFanProcessingTypes", "HauntingType", "affectEntity"),
            ("SMOKE", "fan/processing/AllFanProcessingTypes", "SmokingType", "affectEntity"),
            ("SPLASH", "fan/processing/AllFanProcessingTypes", "SplashingType", "affectEntity"),
            ("MOTION", "fan/AirCurrent", "AirCurrent", "tickAffectedEntities"),
        ]), fan_entity_cases, "Velocity for eligible fixture entities only; media effects, entity filtering, fall and flight counters require game glue.", "M", 1e-6, fan_motion_projection),
    Spec("transport-processing", "TransportProcessingOracle", (
        method("NOTHING", KINETICS + "belt/behaviour/TransportedItemStackHandlerBehaviour", "doesNothing", "class:TransportedResult"),
        method("HELD", KINETICS + "belt/behaviour/TransportedItemStackHandlerBehaviour", "hasHeldOutput", "class:TransportedResult"),
        method("UNCHANGED", KINETICS + "belt/behaviour/TransportedItemStackHandlerBehaviour", "didntChangeFrom", "class:TransportedResult"),
        method("APPLY", KINETICS + "belt/transport/BeltInventory", "applyToEachWithin"),
    ), transport_processing_cases, "Handler distances, callbacks and effective saved item multiset, excluding queue identity/order and host unchanged-result classification."),
    Spec("transport", "TransportOracle", tuple(
        method(marker, KINETICS + "belt/" + path, name, selector)
        for path, entries in [
            ("item/BeltConnectorItem", [("VALIDATE", "validateAxis", None), ("CONNECT", "canConnect", None), ("SLOPE", "getSlopeBetween", None), ("FACING", "getFacingFromTo", None), ("CHAIN", "getBeltChainBetween", None)]),
            ("BeltBlockEntity", [("SPEED", "getBeltMovementSpeed", None), ("AWARE", "getDirectionAwareBeltMovementSpeed", None), ("MOVEMENT_FACING", "getMovementFacing", None), ("CHAIN_DIRECTION", "getBeltChainDirection", None), ("MOVEMENT_DIRECTION", "getMovementDirection", "arity:2")]),
            ("transport/TransportedItemStack", [("OFFSET", "getTargetSideOffset", None), ("COMPARE", "compareTo", None)]),
            ("transport/BeltInventory", [("INSERT", "insert", None), ("BLOCKING", "isBlocking", None), ("CAN_INSERT", "canInsertAtFromSide", None), ("GET_STACK", "getStackAtOffset", None), ("PROCESS", "handleBeltProcessingAndCheckIfRemoved", None), ("TICK", "tick", None)]),
        ] for marker, name, selector in entries), transport_cases,
         "Connector geometry, movement, side insertion and server inventory histories with explicit endpoint and processing callbacks; no native funnels/tunnels/rendering."),
    Spec("burner", "BurnerOracle", tuple(method(marker, "content/processing/burner/BlazeBurnerBlockEntity", name)
        for marker, name in [("HEAT", "getHeatLevel"), ("TICK", "tick"), ("INSERT", "tryUpdateFuel"), ("CREATIVE", "applyCreativeFuel")]),
         burner_cases, "Burner acceptance/fuel/time/creative state only; heat publication, effects and native datamap resolution are outside this adapter.",
         projection=burner_state_projection),
    Spec("network", "NetworkBatch", (), lambda: [" ".join(map(str, v)) for v in itertools.product(
        [-128, -16, 0, 16, 256], [0, 4, 8, 16, 16384], [0, 1, 4])],
         "Unmodified complete KineticNetwork with loaded-identity/getter fixtures; total stress/capacity only."),
    Spec("processing", "ProcessingOracle", (
        method("HELD", KINETICS + "press/BeltPressingCallbacks", "whenItemHeld"),
        method("SPEED", KINETICS + "press/PressingBehaviour", "getRunningTickSpeed"),
        method("HEAD", KINETICS + "press/PressingBehaviour", "getRenderedHeadOffset"),
    ), processing_cases, "Press timing/head curve and held-item routing; deterministic single-output recipe.", "H", 1e-6),
    Spec("placement", "PlacementOracle", (
        method("SHAFT_PREFERRED", KINETICS + "base/RotatedPillarKineticBlock", "getPreferredAxis"),
        method("SHAFT_STATE", KINETICS + "base/RotatedPillarKineticBlock", "getStateForPlacement"),
        method("COG_AXIS", KINETICS + "simpleRelays/CogWheelBlock", "getAxisForPlacement"),
        method("DIRECTION_PREFERRED", KINETICS + "base/DirectionalKineticBlock", "getPreferredFacing"),
        method("DIRECTION_STATE", KINETICS + "base/DirectionalKineticBlock", "getStateForPlacement"),
        method("MOTOR_STATE", KINETICS + "motor/CreativeMotorBlock", "getStateForPlacement"),
    ), placement_cases, "Placement decisions with explicit clicked face, nearest look, shift and shaft neighbours."),
    Spec("rotation", "RotationOracle", (
        method("MODIFIER", KINETICS + "RotationPropagator", "getRotationSpeedModifier"),
        method("AXIS_MODIFIER", KINETICS + "RotationPropagator", "getAxisModifier"),
        method("LARGE_LARGE", KINETICS + "RotationPropagator", "isLargeToLargeGear"),
        method("LARGE_SMALL", KINETICS + "RotationPropagator", "isLargeToSmallCog"),
        method("CLUTCH", KINETICS + "transmission/ClutchBlockEntity", "getRotationSpeedModifier"),
        method("GEARSHIFT", KINETICS + "transmission/GearshiftBlockEntity", "getRotationSpeedModifier"),
    ), rotation_cases, "Local gear/shaft ratios and explicit source-relative port bases; not network propagation.", "01234567", 0.0),
    Spec("milling", "MillingOracle", (
        method("SPEED", KINETICS + "millstone/MillstoneBlockEntity", "getProcessingSpeed"),
        method("TICK", KINETICS + "millstone/MillstoneBlockEntity", "tick"),
        method("PROCESS", KINETICS + "millstone/MillstoneBlockEntity", "process"),
        method("ROLL", "content/processing/recipe/ProcessingOutput", "rollOutput"),
    ), milling_cases, "Millstone ticks with fixture inventory/recipe lookup, speed and seeded result counts (PRNG implementations differ).", projection=milling_state_projection),
    Spec("drilling", "DrillingOracle", (
        method("BREAKABLE", KINETICS + "base/BlockBreakingKineticBlockEntity", "isBreakable"),
        method("SPEED", KINETICS + "base/BlockBreakingKineticBlockEntity", "getBreakSpeed"),
        method("WAKE", KINETICS + "base/BlockBreakingKineticBlockEntity", "destroyNextTick"),
        method("TICK", KINETICS + "base/BlockBreakingKineticBlockEntity", "tick"),
        method("DAMAGE", KINETICS + "drill/DrillBlock", "getDamage"),
    ), drilling_cases, "Drill break/damage timing, lazy ticks, wake and pause/resume; fixture world IO.", "SD", 1e-5),
    Spec("visual", "VisualOracle", (
        method("ANGLE", KINETICS + "base/KineticBlockEntityRenderer", "getAngleForBe"),
        method("OFFSET_TEST", KINETICS + "base/KineticBlockEntityVisual", "shouldOffset"),
        method("OFFSET", KINETICS + "base/KineticBlockEntityVisual", "rotationOffset"),
        method("SPEED", KINETICS + "gauge/SpeedGaugeBlockEntity", "getDialTarget"),
        method("ANGULAR", KINETICS + "base/KineticBlockEntity", "convertToAngular"),
        method("CRANK", KINETICS + "crank/HandCrankBlockEntity", "tick"),
        method("CRANK_ANGLE", KINETICS + "crank/HandCrankBlockEntity", "getIndependentAngle"),
        method("WHEEL", KINETICS + "waterwheel/WaterWheelBlockEntity", "getGeneratedSpeed"),
    ), visual_cases, "Animation math and gauge targets in fixture contexts; not rendered pixels or UI parity.", "OASCW", 1e-4),
    Spec("fan", "FanOracle", (
        method("RANGE", KINETICS + "fan/IAirCurrentSource", "getMaxDistance"),
        method("DIRECTION", KINETICS + "fan/EncasedFanBlockEntity", "getAirFlowDirection"),
        method("CONVERT", KINETICS + "base/KineticBlockEntity", "convertToDirection"),
        method("PROPELLER", KINETICS + "fan/FanVisual", "getFanSpeed"),
        method("CAN_PROCESS", KINETICS + "fan/processing/FanProcessing", "canProcess"),
        method("DROPPED", KINETICS + "fan/processing/FanProcessing", "applyProcessing", "first:ItemEntity"),
        method("TRANSPORTED", KINETICS + "fan/processing/FanProcessing", "applyProcessing", "first:TransportedItemStack"),
        method("DECREMENT", KINETICS + "fan/processing/FanProcessing", "decrementProcessingTime"),
    ), fan_cases, "Fan range/direction/propeller and complete item processing timer histories; fixture recipes/NBT.", "MV", 1e-6),
    Spec("fan-media", "FanMediaOracle", tuple(
        method(kind, KINETICS + "fan/processing/AllFanProcessingTypes", "isValidAt", "class:" + kind)
        for kind in ["BlastingType", "HauntingType", "SmokingType", "SplashingType"]
    ), lambda: [" ".join(map(str, v)) for v in itertools.product(
        range(16), range(16), range(2), range(3), range(3), range(6))],
         "Exhaustive custom catalyst tags/properties, individual validity masks and priority selection."),
    Spec("basin", "BasinOracle", (
        method("APPLY", "content/processing/basin/BasinRecipe", "apply", "arity:3"),
        method("INSERT", "content/processing/basin/BasinInventory", "insertItem", "arity:3"),
        method("ACCEPT", "content/processing/basin/BasinBlockEntity", "acceptOutputs", "arity:3"),
        method("ACCEPT_INNER", "content/processing/basin/BasinBlockEntity", "acceptOutputsInner", "arity:3"),
        method("ACCEPT_FLUID", "content/processing/basin/BasinBlockEntity", "acceptFluidOutputsIntoBasin", "arity:3"),
        method("ACCEPT_ITEM", "content/processing/basin/BasinBlockEntity", "acceptItemOutputsIntoBasin", "arity:3"),
        method("FILL", "foundation/fluid/CombinedTankWrapper", "fill", "arity:2"),
        method("ROLL", "content/processing/recipe/ProcessingOutput", "rollOutput", "arity:1"),
        method("DUMMY", "content/processing/basin/BasinRecipe", "getCraftingInput", "arity:2"),
        method("HEAT", "content/processing/recipe/HeatCondition", "testBlazeBurner", "arity:1"),
        method("TICK", KINETICS + "mixer/MechanicalMixerBlockEntity", "tick", "arity:0"),
        method("START", KINETICS + "mixer/MechanicalMixerBlockEntity", "startProcessingBasin", "arity:0"),
        method("REMOVE", KINETICS + "mixer/MechanicalMixerBlockEntity", "onBasinRemoved", "arity:0"),
    ), lambda: basin_cases()[0], "Basin apply success/inventories in stationary contexts and mixer histories; host RNG/notification counters are not observed.", projection=basin_state_projection),
)
