package dev.createfuzz;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.simibubi.create.content.kinetics.base.IRotate;
import com.simibubi.create.content.kinetics.base.KineticBlockEntity;
import com.simibubi.create.content.kinetics.gauge.GaugeBlock;
import com.simibubi.create.content.kinetics.transmission.SplitShaftBlockEntity;
import com.simibubi.create.content.kinetics.transmission.sequencer.SequencerInstructions;

import net.minecraft.commands.arguments.blocks.BlockStateParser;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;

import java.util.ArrayList;
import java.util.List;

/** Normalized semantic observations; saved NBT is diagnostic only. */
final class Observations {
    static JsonObject snapshot(ServerLevel level, BlockPos origin, JsonObject c, net.minecraft.server.level.ServerPlayer actor) {
        JsonObject blocks = new JsonObject();
        var networks = new java.util.TreeMap<Long, java.util.List<String>>();
        JsonObject sizes = new JsonObject();
        List<JsonObject> all = new ArrayList<>();
        for (JsonElement b : c.getAsJsonArray("blocks")) all.add(b.getAsJsonObject());
        if (c.has("events"))
            for (JsonElement b : c.getAsJsonArray("events")) all.add(b.getAsJsonObject());
        int[] lo = {24, 24, 24}, hi = {-24, -24, -24};
        for (JsonObject b : all)
            for (int a = 0; a < 3; a++) {
                int v = b.getAsJsonArray("pos").get(a).getAsInt();
                lo[a] = Math.min(lo[a], v);
                hi[a] = Math.max(hi[a], v);
            }
        boolean transportVersion=c.has("version") && c.get("version").getAsInt()>=3;
        int padding=FluidBounds.enabled(c)?2:1;
        for (int a = 0; a < 3; a++) {
            lo[a]-=padding;
            hi[a]+=padding;
        }
        if (!all.isEmpty()
                && (lo[0] < -24
                        || lo[2] < -24
                        || lo[1] < 0
                        || hi[0] > 24
                        || hi[2] > 24
                        || hi[1] > 24)) throw new IllegalArgumentException("region exceeds slot");
        for (int x = lo[0]; x <= hi[0]; x++)
            for (int y = lo[1]; y <= hi[1]; y++)
                for (int z = lo[2]; z <= hi[2]; z++) {
                    String key = x + "," + y + "," + z;
                    BlockPos pos = origin.offset(x, y, z);
                    BlockState state = level.getBlockState(pos);
                    JsonObject snap = new JsonObject();
                    snap.addProperty("state", BlockStateParser.serialize(state));
                    var fs = state.getFluidState();
                    if (!fs.isEmpty()) {
                        var fluid = new JsonObject();
                        String id = net.minecraft.core.registries.BuiltInRegistries.FLUID.getKey(fs.getType()).toString().replace(":flowing_", ":");
                        fluid.addProperty("fluid", id);
                        fluid.addProperty("source", fs.isSource());
                        fluid.addProperty("amount", fs.getAmount());
                        fluid.addProperty("falling", fs.hasProperty(net.minecraft.world.level.material.FlowingFluid.FALLING) && fs.getValue(net.minecraft.world.level.material.FlowingFluid.FALLING));
                        snap.add("world_fluid", fluid);
                    }
                    BlockEntity be = level.getBlockEntity(pos);
                    FluidMachineObservations.add(be,snap);
                    var fluidNetwork=FluidObservations.observe(level,pos);if(fluidNetwork!=null)snap.add("fluid_network",fluidNetwork);
                    if (be instanceof com.simibubi.create.content.fluids.tank.FluidTankBlockEntity tank) {
                        var cbe = tank.getControllerBE();
                        if (cbe != null) {
                            var t = new JsonObject();
                            var cp = tank.getController().subtract(origin);
                            t.add("controller", CreateFuzz.GSON.toJsonTree(new int[]{cp.getX(), cp.getY(), cp.getZ()}));
                            t.addProperty("width", cbe.getWidth());
                            t.addProperty("height", cbe.getHeight());
                            var inv = cbe.getTankInventory();
                            t.addProperty("capacity", inv.getCapacity());
                            t.addProperty("amount", inv.getFluidAmount());
                            if (!inv.getFluid().isEmpty()) t.addProperty("fluid", FluidIds.of(inv.getFluid()));
                            snap.add("fluid_tank", t);
                        }
                    }
                    if (be != null) {var transport=TransportObservations.observe(be);if(transport!=null) snap.add("transport",transport); }
                    if (be != null) {
                        var tunnel=TransportObservations.tunnel(be);if(tunnel!=null) snap.add("tunnel",tunnel);
                        var funnel=TransportObservations.funnel(be);if(funnel!=null) snap.add("funnel",funnel);
                        var chute=TransportObservations.chute(be);if(chute!=null) snap.add("chute",chute);
                        // Ejectors' plates and throws belong to the version 3 transport contract; version 1
                        // kinetic cases use ejectors as stress loads only.
                        var ejector=transportVersion?TransportObservations.ejector(be):null;if(ejector!=null) snap.add("ejector",ejector);
                    }
                    if (be instanceof com.simibubi.create.content.kinetics.belt.BeltBlockEntity belt) {
                        JsonObject b = new JsonObject();
                        BlockPos controller = belt.getController().subtract(origin);
                        b.add("controller",CreateFuzz.GSON.toJsonTree(new int[]{controller.getX(),controller.getY(),controller.getZ()}));
                        b.addProperty("length",belt.beltLength);
                        b.addProperty("index",belt.index);
                        b.addProperty("casing",belt.casing.name().toLowerCase(java.util.Locale.ROOT));
                        b.addProperty("covered",belt.covered);
                        b.add("color",belt.color.map(dye->(com.google.gson.JsonElement)new com.google.gson.JsonPrimitive(dye.getSerializedName())).orElse(com.google.gson.JsonNull.INSTANCE));
                        snap.add("belt",b);
                    }
                    if (be instanceof com.simibubi.create.content.kinetics.chainConveyor.ChainConveyorBlockEntity conveyor) {
                        var links=new java.util.ArrayList<int[]>();
                        for(var q:conveyor.connections) links.add(new int[]{q.getX(),q.getY(),q.getZ()});
                        links.sort(java.util.Arrays::compare);
                        snap.add("conveyor_links",CreateFuzz.GSON.toJsonTree(links));
                    }
                    if (be instanceof KineticBlockEntity k) {
                        snap.add("saved", Persistence.saved(k, origin));
                        snap.add("kinetic", kinetic(k));
                        if (k.network != null) {
                            networks.computeIfAbsent(k.network, ignored -> new ArrayList<>())
                                    .add(key);
                            sizes.addProperty(key, networkSize(k));
                        }
                    }
                    var bracket =
                            com.simibubi.create.foundation.blockEntity.behaviour
                                    .BlockEntityBehaviour.get(
                                    level,
                                    pos,
                                    com.simibubi.create.content.decoration.bracket
                                            .BracketedBlockEntityBehaviour.TYPE);
                    if (bracket != null && bracket.getBracket() != null)
                        snap.addProperty(
                                "bracket", BlockStateParser.serialize(bracket.getBracket()));
                    // For reading mismatches only; never compared.
                    if (be != null && !(be instanceof com.simibubi.create.content.kinetics.belt.BeltBlockEntity))
                        snap.addProperty(
                                "debug", be.saveWithoutMetadata(level.registryAccess()).toString());
                    if(be!=null) {var payload=Persistence.payload(be,origin);if(payload!=null) snap.add("payload",payload);}
                    blocks.add(key, snap);
                }
        JsonObject out = new JsonObject();
        out.addProperty("side", "java");
        out.add("origin", CreateFuzz.GSON.toJsonTree(new int[]{origin.getX(), origin.getY(), origin.getZ()}));
        out.add("save_phase",c.get("_save_phase"));
        out.add("item_outcomes",ItemInputs.outcomes(actor));
        out.add("blocks", blocks);
        var partitions = new ArrayList<java.util.List<String>>(networks.values());
        for (var group : partitions) java.util.Collections.sort(group);
        partitions.sort(java.util.Comparator.comparing(group -> group.get(0)));
        JsonObject normalized = new JsonObject();
        normalized.add("partitions", CreateFuzz.GSON.toJsonTree(partitions));
        normalized.add("sizes", sizes);
        out.add("networks", normalized);
        return out;
    }

    /** Read the registry without getOrCreateNetworkFor: observing must never mutate it. */
    @SuppressWarnings("unchecked")
    static int networkSize(KineticBlockEntity k) {
        try {
            var f =
                    com.simibubi.create.content.kinetics.TorquePropagator.class.getDeclaredField(
                            "networks");
            f.setAccessible(true);
            var worlds =
                    (java.util.Map<
                                    net.minecraft.world.level.LevelAccessor,
                                    java.util.Map<
                                            Long,
                                            com.simibubi.create.content.kinetics.KineticNetwork>>)
                            f.get(null);
            var networks = worlds.get(k.getLevel());
            var network = networks == null ? null : networks.get(k.network);
            if (network == null)
                throw new IllegalStateException("kinetic entity names an absent network");
            return network.getSize();
        } catch (ReflectiveOperationException ex) {
            throw new RuntimeException(ex);
        }
    }

    static JsonObject kinetic(KineticBlockEntity k) {
        JsonObject o = new JsonObject();
        o.addProperty("speed", k.getSpeed());
        if (k instanceof com.simibubi.create.content.kinetics.crank.ValveHandleBlockEntity valve) {
            JsonObject v=new JsonObject();
            v.addProperty("selected",valve.angleInput.getValue());
            v.addProperty("in_use",valve.inUse);
            // The externally driven branch calls a client renderer, unavailable on a server.
            if (!(valve.inUse == 0 && valve.hasSource() && valve.getSpeed() != 0))
                v.addProperty("angle",valve.getIndependentAngle(0));
            else v.addProperty("angle_unavailable","externally driven angle requires client renderer");
            o.add("valve",v);
        }
        if (k instanceof com.simibubi.create.content.kinetics.chainDrive.ChainGearshiftBlockEntity chain)
            o.addProperty("chain_modifier", chain.getModifier());
        if (k instanceof com.simibubi.create.content.kinetics.motor.CreativeMotorBlockEntity motor)
            o.addProperty("speed_setting", motor.generatedSpeed.getValue());
        if (k instanceof com.simibubi.create.content.kinetics.speedController.SpeedControllerBlockEntity controller)
            o.addProperty("speed_setting", controller.targetSpeed.getValue());
        o.addProperty("theoretical", k.getTheoreticalSpeed());
        o.addProperty("overstressed", k.isOverStressed());
        o.addProperty("stress", field(k, "stress"));
        o.addProperty("capacity", field(k, "capacity"));
        // calculateStressApplied/calculateAddedStressCapacity also cache their result in the
        // entity (lastStressApplied, lastCapacityProvided), which write() saves: observing must
        // not change what Create later saves, so the caches are put back.
        float lastStress = field(k, "lastStressApplied");
        float lastCapacity = field(k, "lastCapacityProvided");
        o.addProperty("impact", k.calculateStressApplied());
        o.addProperty("added_capacity", k.calculateAddedStressCapacity());
        setField(k, "lastStressApplied", lastStress);
        setField(k, "lastCapacityProvided", lastCapacity);
        o.addProperty("generated", k.getGeneratedSpeed());
        o.addProperty("has_source", k.hasSource());
        if (k.getBlockState().getBlock() instanceof GaugeBlock gauge)
            o.addProperty(
                    "comparator",
                    gauge.getAnalogOutputSignal(k.getBlockState(), k.getLevel(), k.getBlockPos()));
        if (k.sequenceContext != null) {
            String name =
                    k.sequenceContext.instruction() == SequencerInstructions.TURN_ANGLE
                            ? "sequence_angle"
                            : "sequence_distance";
            o.addProperty(name, k.sequenceContext.getEffectiveValue(k.getTheoreticalSpeed()));
        }
        if (k instanceof com.simibubi.create.content.kinetics.transmission.sequencer.SequencedGearshiftBlockEntity) {
            var tag = k.saveWithoutMetadata(k.getLevel().registryAccess());
            JsonObject run = new JsonObject();
            run.addProperty("current", tag.getInt("InstructionIndex"));
            run.addProperty("duration", tag.getInt("InstructionDuration"));
            run.addProperty("progress", tag.getFloat("InstructionProgress"));
            run.addProperty("timer", tag.getInt("Timer"));
            run.addProperty("powered_before", tag.getBoolean("PrevPowered"));
            o.add("sequencer", run);
        }
        // A clutch's or gearshift's halves: the speed each shaft face turns at.
        if (k instanceof SplitShaftBlockEntity s
                && k.getBlockState().getBlock() instanceof IRotate r) {
            JsonObject faces = new JsonObject();
            Direction.Axis axis = r.getRotationAxis(k.getBlockState());
            for (Direction d : Direction.values())
                if (d.getAxis() == axis)
                    faces.addProperty(
                            d.getSerializedName(),
                            s.getSpeed() == 0 ? 0f : s.getSpeed() * s.getRotationSpeedModifier(d));
            o.add("faces", faces);
        }
        return o;
    }

    static void setField(KineticBlockEntity k, String name, float value) {
        try {
            java.lang.reflect.Field f = KineticBlockEntity.class.getDeclaredField(name);
            f.setAccessible(true);
            f.setFloat(k, value);
        } catch (ReflectiveOperationException ex) {
            throw new RuntimeException(ex);
        }
    }

    static float field(KineticBlockEntity k, String name) {
        try {
            java.lang.reflect.Field f = KineticBlockEntity.class.getDeclaredField(name);
            f.setAccessible(true);
            return f.getFloat(k);
        } catch (ReflectiveOperationException ex) {
            throw new RuntimeException(ex);
        }
    }
}
