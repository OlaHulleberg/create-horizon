package dev.createfuzz;

import com.google.gson.*;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.commands.arguments.blocks.BlockStateParser;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.storage.ServerLevelData;

/** Native environmental fluid ticks for the Rust driver, in a suspended per-case namespace.
 * Block entities and scheduled block ticks do not run here. Create's fluid network is run
 * exclusively by Rust; only the public native fluid tick scheduler advances. */
final class FluidHost {
    private static final BlockPos ORIGIN = new BlockPos(2048, 0, 2048);
    // Only one namespace is materialized at a time, just like ordinary Java cases.
    // Suspended peers have no blocks, entities or ticks in the shared level.
    private static final java.util.Map<String, Session> sessions = new java.util.LinkedHashMap<>();
    private static final class Session {
        long epoch;
        final net.minecraft.util.RandomSource random = net.minecraft.util.RandomSource.create(0);
        net.minecraft.server.level.ServerPlayer actor;
        final java.util.Map<BlockPos, net.minecraft.world.level.block.state.BlockState> blocks = new java.util.LinkedHashMap<>();
        final java.util.Map<BlockPos, net.minecraft.nbt.CompoundTag> blockEntities = new java.util.LinkedHashMap<>();
        final java.util.List<net.minecraft.world.ticks.ScheduledTick<net.minecraft.world.level.material.Fluid>> ticks = new java.util.ArrayList<>();
    }
    static void reapStale(ServerLevel level) {
        for (String key : new java.util.ArrayList<>(sessions.keySet())) {
            try {
                long pid = Long.parseLong(key.substring(0, key.indexOf('-')));
                if (!ProcessHandle.of(pid).map(ProcessHandle::isAlive).orElse(false)) abortIfOwner(level, key);
            } catch (NumberFormatException | IndexOutOfBoundsException ignored) { }
        }
    }
    static void prepare(ServerLevel level) {
        for(int x=(ORIGIN.getX()-24)>>4;x<=(ORIGIN.getX()+24)>>4;x++)
            for(int z=(ORIGIN.getZ()-24)>>4;z<=(ORIGIN.getZ()+24)>>4;z++) level.setChunkForced(x,z,true);
    }
    static void abortIfOwner(ServerLevel level, String key) {
        Session session = sessions.remove(key);
        if (session != null) PlayerActor.release(level, session.actor);
    }
    static JsonObject handle(ServerLevel level, JsonObject request) throws Exception {
        reapStale(level);
        String key=request.get("owner").getAsString();
        if (request.has("begin")) {
            if (sessions.containsKey(key)) throw new IllegalStateException("fluid host duplicate begin");
            Session session = new Session();
            session.epoch = level.getGameTime();
            session.actor = PlayerActor.create(level,16);
            sessions.put(key, session);
        }
        Session session = sessions.get(key);
        if (session == null) throw new IllegalStateException("fluid host owner mismatch");
        if (request.has("close")) {
            abortIfOwner(level,key);
            return new JsonObject();
        }
        CaseIsolation.begin(level);
        long previousTime = level.getGameTime();
        var randomField = net.minecraft.world.level.Level.class.getDeclaredField("random");
        randomField.setAccessible(true);
        var previousRandom = level.random;
        boolean complete = false;
        try {
            randomField.set(level,session.random);
            // No neighbour notifications while restoring a previously observed frame.
            for (var entry : session.blocks.entrySet()) level.setBlock(entry.getKey(),entry.getValue(),18);
            for (var entry : session.blockEntities.entrySet()) {
                var entity = level.getBlockEntity(entry.getKey());
                if (entity == null) throw new IllegalStateException("fluid host lost block entity at " + entry.getKey());
                entity.loadWithComponents(entry.getValue(),level.registryAccess());
            }
            var bounds = new BoundingBox(ORIGIN.getX()-24,level.getMinBuildHeight(),ORIGIN.getZ()-24,
                    ORIGIN.getX()+24,level.getMaxBuildHeight(),ORIGIN.getZ()+24);
            level.getFluidTicks().clearArea(bounds);
            for (var scheduled : session.ticks) level.getFluidTicks().schedule(scheduled);
            JsonObject result = frame(level,request,session);
            CaseIsolation.assertContained(level,ORIGIN);
            session.blocks.clear();
            session.blockEntities.clear();
            for (var pos : BlockPos.betweenClosed(ORIGIN.offset(-24,0,-24),ORIGIN.offset(24,24,24))) {
                var state = level.getBlockState(pos);
                if (!state.isAir()) session.blocks.put(pos.immutable(),state);
                var entity = level.getBlockEntity(pos);
                if (entity != null) session.blockEntities.put(pos.immutable(),entity.saveWithFullMetadata(level.registryAccess()));
            }
            session.ticks.clear();
            for(int x=(ORIGIN.getX()-24)>>4;x<=(ORIGIN.getX()+24)>>4;x++)
                for(int z=(ORIGIN.getZ()-24)>>4;z<=(ORIGIN.getZ()+24)>>4;z++)
                    ((net.minecraft.world.ticks.LevelChunkTicks<net.minecraft.world.level.material.Fluid>)level.getChunk(x,z).getFluidTicks()).getAll().forEach(session.ticks::add);
            complete = true;
            return result;
        } finally {
            try { CaseIsolation.assertContained(level,ORIGIN); }
            finally {
                try {
                    if (complete) CaseIsolation.clear(level,ORIGIN);
                    else CaseIsolation.clearRejected(level,ORIGIN);
                }
                finally {
                    randomField.set(level,previousRandom);
                    ((ServerLevelData)level.getLevelData()).setGameTime(previousTime);
                }
            }
            CaseIsolation.empty(level);
        }
    }
    private static JsonObject frame(ServerLevel level, JsonObject request, Session session) throws Exception {
        long epoch = session.epoch;
        var actor = session.actor;
        long clock=request.get("clock").getAsLong();
        var data=(ServerLevelData)level.getLevelData();
        data.setGameTime(epoch+clock-(request.has("bucket")?0:1));
        for(var value:request.getAsJsonArray("changes")) {
            var change=value.getAsJsonObject();var xyz=change.getAsJsonArray("pos");
            var pos=ORIGIN.offset(xyz.get(0).getAsInt(),xyz.get(1).getAsInt(),xyz.get(2).getAsInt());
            var state=BlockStateParser.parseForBlock(BuiltInRegistries.BLOCK.asLookup(),change.get("state").getAsString(),false).blockState();
            level.setBlock(pos,state,3);
        }
        if(request.has("bucket")) {
            WorldBucketInput.apply(level,actor,ORIGIN,request.getAsJsonObject("bucket"));
        } else {
            for(var value:request.getAsJsonArray("native")) {
                var xyz=value.getAsJsonArray();var pos=ORIGIN.offset(xyz.get(0).getAsInt(),xyz.get(1).getAsInt(),xyz.get(2).getAsInt());
                var state=level.getFluidState(pos);if(!state.isEmpty()) level.scheduleTick(pos,state.getType(),state.getType().getTickDelay(level));
            }
            data.setGameTime(epoch+clock);
            level.getFluidTicks().tick(epoch+clock,65536,(pos,fluid)-> {
                var state=level.getFluidState(pos);
                if(state.getType()==fluid) state.tick(level,pos);
            });
            for(var value:request.getAsJsonArray("manual")) {
                var xyz=value.getAsJsonArray();var pos=ORIGIN.offset(xyz.get(0).getAsInt(),xyz.get(1).getAsInt(),xyz.get(2).getAsInt());
                var state=level.getFluidState(pos);if(!state.isEmpty()) state.tick(level,pos);
            }
            FluidBounds.check(level,ORIGIN,request.getAsJsonArray("region"));
        }
        // The native flow vector at the requested cells (`FluidState.getFlow`), which water
        // wheels read through `WaterWheelBlockEntity.getFlowVectorAtPosition`. Queried after
        // this frame's fluid ticks, like the level's own scheduled block ticks.
        var flows = new JsonArray();
        for (var value : request.getAsJsonArray("flows")) {
            var xyz=value.getAsJsonArray();
            var pos=ORIGIN.offset(xyz.get(0).getAsInt(),xyz.get(1).getAsInt(),xyz.get(2).getAsInt());
            var flow=level.getFluidState(pos).getFlow(level,pos);
            flows.add(CreateFuzz.GSON.toJsonTree(new double[]{flow.x,flow.y,flow.z}));
        }
        CaseIsolation.disposeUnobservedDrops(level,ORIGIN);
        var out=new JsonObject();var states=new JsonArray();
        for(var value:request.getAsJsonArray("region")) {
            var xyz=value.getAsJsonArray();var pos=ORIGIN.offset(xyz.get(0).getAsInt(),xyz.get(1).getAsInt(),xyz.get(2).getAsInt());
            var entry=new JsonObject();entry.add("pos",xyz);var state=level.getBlockState(pos);entry.addProperty("state",BlockStateParser.serialize(state));
            var facts=new JsonObject();facts.addProperty("replaceable",state.canBeReplaced());facts.addProperty("collision",!state.getCollisionShape(level,pos).isEmpty());
            int sturdy=0;for(var direction:net.minecraft.core.Direction.values())if(state.isFaceSturdy(level,pos,direction))sturdy|=1<<direction.get3DDataValue();facts.addProperty("sturdy",sturdy);
            var fs=state.getFluidState();if(!fs.isEmpty()){var fluid=new JsonObject();fluid.addProperty("fluid",BuiltInRegistries.FLUID.getKey(fs.getType()).toString().replace(":flowing_",":"));fluid.addProperty("source",fs.isSource());fluid.addProperty("amount",fs.getAmount());fluid.addProperty("falling",fs.hasProperty(net.minecraft.world.level.material.FlowingFluid.FALLING)&&fs.getValue(net.minecraft.world.level.material.FlowingFluid.FALLING));facts.add("fluid",fluid);}
            entry.add("facts",facts);states.add(entry);
        }
        out.add("states",states);
        var rules=new JsonObject();
        rules.addProperty("water_source_conversion",level.getGameRules().getBoolean(net.minecraft.world.level.GameRules.RULE_WATER_SOURCE_CONVERSION));
        rules.addProperty("lava_source_conversion",level.getGameRules().getBoolean(net.minecraft.world.level.GameRules.RULE_LAVA_SOURCE_CONVERSION));
        rules.addProperty("evaporates",level.dimensionType().ultraWarm());out.add("rules",rules);out.add("flows",flows);
        return out;
    }
}
