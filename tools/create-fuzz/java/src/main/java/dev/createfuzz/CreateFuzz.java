package dev.createfuzz;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.simibubi.create.content.kinetics.crank.HandCrankBlockEntity;
import com.simibubi.create.content.kinetics.motor.CreativeMotorBlockEntity;

import net.minecraft.commands.arguments.blocks.BlockStateParser;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.neoforged.bus.api.IEventBus;
import net.neoforged.fml.common.Mod;
import net.neoforged.neoforge.common.NeoForge;
import net.neoforged.neoforge.event.server.ServerStartedEvent;
import net.neoforged.neoforge.event.tick.ServerTickEvent;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.List;

/**
 * Runs fuzzing scenarios on a dedicated server without restarting it. A batch is a JSON array of
 * scenarios dropped into {@code <dir>/inbox/<name>.json}; each case gets its own area, its blocks
 * are placed one per tick (block i before tick i), cases run in separate, serial level namespaces, and the
 * snapshots are written to {@code <dir>/outbox/<name>.json} in the batch's order. The directory is
 * {@code $FUZZ_DIR}, or {@code fuzz} in the server's directory.
 */
@Mod("createfuzz")
public class CreateFuzz {
    static final Gson GSON = new GsonBuilder().disableHtmlEscaping().create();

    /** Cases sit on a grid, this far apart, at this height (above a flat world's floor). */
    static final int SPACING = 64, Y = 0, CLEAR_RADIUS = 24, CLEAR_HEIGHT = 24;

    final Path dir;
    Batch batch;

    /**
     * The areas cases run in: force-loaded once at start, so no chunk loads while cases run (at a
     * sprint tick rate chunk loading lags, and block entities placed in a chunk not yet in the
     * level wait to tick). A batch holds at most this many cases.
     */
    static final int SLOTS = 128;

    int startup = -1;
    final RequestQueue<Batch> requests = new RequestQueue<>();

    public CreateFuzz(IEventBus modBus) {
        SignalBlock.REGISTRY.register(modBus);
        String env = System.getenv("FUZZ_DIR");
        dir = Path.of(env != null ? env : "fuzz");
        NeoForge.EVENT_BUS.addListener(this::started);
        NeoForge.EVENT_BUS.addListener(this::tick);
    }

    void started(ServerStartedEvent e) {
        try {
            Catalogue.audit();
            ItemInputs.audit();
            FluidRecipeAudit.check(e.getServer().overworld());
            Files.createDirectories(dir);
            Files.writeString(
                    dir.resolve("actor-probe.json"),
                    GSON.toJson(PlayerActor.probe(e.getServer().overworld())));
            Files.writeString(dir.resolve("default-states.json"), DefaultStates.json());
            Files.writeString(
                    dir.resolve("runtime-probe.json"),
                    GSON.toJson(RuntimeProbe.angles(e.getServer().overworld())));
            Files.createDirectories(dir.resolve("inbox"));
            Files.createDirectories(dir.resolve("outbox"));
            ServerLevel level = e.getServer().overworld();
            for (int i = 0; i < SLOTS; i++) {
                BlockPos o = slot(i);
                for (int cx = (o.getX() - CLEAR_RADIUS) >> 4;
                        cx <= (o.getX() + CLEAR_RADIUS) >> 4;
                        cx++)
                    for (int cz = (o.getZ() - CLEAR_RADIUS) >> 4;
                            cz <= (o.getZ() + CLEAR_RADIUS) >> 4;
                            cz++) level.setChunkForced(cx, cz, true);
            }
            // The native peer advances scheduled fluids, not ambient random block ticks.
            // Use the same deterministic external environment for ordinary Java cases.
            level.getGameRules().getRule(net.minecraft.world.level.GameRules.RULE_RANDOMTICKING).set(0,e.getServer());
            level.getGameRules().getRule(net.minecraft.world.level.GameRules.RULE_DOFIRETICK).set(false,e.getServer());
            FluidHost.prepare(level);
            // Run ticks as fast as the server can.
            e.getServer().tickRateManager().setTickRate(10000f);
            startup = 0;
        } catch (IOException ex) {
            throw new RuntimeException(ex);
        }
    }

    static BlockPos slot(int i) {
        return new BlockPos((i % 16) * SPACING, Y, (i / 16) * SPACING);
    }

    void tick(ServerTickEvent.Post e) {
        MinecraftServer server = e.getServer();
        ServerLevel level = server.overworld();
        // Ready once every slot's blocks tick, and 100 ticks more for their chunks to settle.
        if (startup >= 0) {
            boolean all = true;
            for (int i = 0; i < SLOTS && all; i++) all = level.isPositionEntityTicking(slot(i));
            if (all && ++startup > 100) {
                startup = -1;
                try {
                    for (int i = 0; i < SLOTS; i++) CaseIsolation.clear(level, slot(i));
                    CaseIsolation.initialize(level, dir);
                    Files.writeString(dir.resolve("ready"), "ready\n");
                } catch (Exception ex) {
                    throw new RuntimeException(ex);
                }
            }
            return;
        }
        if (batch == null) {
            requests.discover(dir.resolve("inbox"));
            var request = requests.poll();
            if (request == null) return;
            if (request.value() != null) {
                batch = request.value();
                return;
            }
            Path next = request.path();
            com.google.gson.JsonElement input=null;
            try {
                input=JsonParser.parseString(Files.readString(next));
                Files.delete(next);
                if(next.getFileName().toString().startsWith("fluid-host-")) {
                    var result=FluidHost.handle(level,input.getAsJsonObject());
                    writeResponse(dir.resolve("outbox").resolve(next.getFileName()),result);
                    return;
                }
                FluidHost.reapStale(level);
                server.tickRateManager().setFrozen(false);
                JsonArray cases=input.getAsJsonArray();
                batch = new Batch(next.getFileName().toString(), cases, level, dir);

                batch.start(level);
                if (batch.name.startsWith("cover-")) coverage(null);
            } catch (Exception ex) {
                try {
                    if(next.getFileName().toString().startsWith("fluid-host-") && input!=null && input.isJsonObject() && input.getAsJsonObject().has("owner")) FluidHost.abortIfOwner(level,input.getAsJsonObject().get("owner").getAsString());
                } catch (Exception cleanup) { ex.addSuppressed(cleanup); }
                fail(level, next.getFileName().toString(), ex);
            }
            return;
        }
        try {
            // Yield only after disposal has settled, before materializing another case.
            // No live world state or actors are suspended with the batch.
            if (!batch.yielded && batch.settling == 1 && batch.activeCase > 0
                    && batch.activeCase < batch.cases.size()) {
                CaseIsolation.empty(level);
                requests.discover(dir.resolve("inbox"));
                batch.yielded = true;
                requests.resume(batch);
                batch = null;
                return;
            }
            if (batch.step(level)) {
                CaseIsolation.empty(level);
                batch.releaseActors(level);
                if (batch.name.startsWith("cover-") && !batch.coverageCaptured)
                    coverage(dir.resolve("outbox").resolve(batch.name + ".exec"));
                writeResponse(dir.resolve("outbox").resolve(batch.name),batch.results);
                batch = null;
            }
        } catch (Exception ex) {
            fail(level, batch.name, ex);
        }
    }

    /**
     * Reset after cleanup, dump before publishing the batch response. No agent dependency in the
     * mod.
     */
    static void coverage(Path output) {
        try {
            Object agent =
                    Class.forName(
                                    "org.jacoco.agent.rt.RT",
                                    true,
                                    ClassLoader.getSystemClassLoader())
                            .getMethod("getAgent")
                            .invoke(null);
            if (output == null) agent.getClass().getMethod("reset").invoke(agent);
            else
                Files.write(
                        output,
                        (byte[])
                                agent.getClass()
                                        .getMethod("getExecutionData", boolean.class)
                                        .invoke(agent, true));
        } catch (Exception ex) {
            throw new RuntimeException("coverage requires the JaCoCo agent", ex);
        }
    }

    static void writeResponse(Path output,JsonElement value) throws IOException {
        Path tmp=output.resolveSibling(output.getFileName()+".tmp");
        try(var writer=Files.newBufferedWriter(tmp,StandardCharsets.UTF_8)){GSON.toJson(value,writer);}
        Files.move(tmp,output,StandardCopyOption.ATOMIC_MOVE);
    }

    /** A batch that could not run: its answer is the error, and its area is cleared. */
    void fail(ServerLevel level, String name, Exception ex) {
        // Detach before disposing: a secondary cleanup failure must remain request-local.
        Batch failed = batch;
        batch = null;
        if (failed != null) {
            try {
                for (BlockPos origin : failed.origins) CaseIsolation.clearRejected(level, origin);
                CaseIsolation.empty(level);
            } catch (Exception cleanup) { ex.addSuppressed(cleanup); }
            finally {
                try { failed.releaseActors(level); }
                catch (Exception cleanup) { ex.addSuppressed(cleanup); }
            }
        }
        try {
            Files.deleteIfExists(dir.resolve("inbox").resolve(name));
            JsonObject o = new JsonObject();
            o.addProperty("error", ex + (ex.getCause() != null ? ": " + ex.getCause() : ""));
            writeResponse(dir.resolve("outbox").resolve(name),o);
            System.err.println("fuzz request failed: "+name+": "+ex);
            for (var cleanup : ex.getSuppressed()) System.err.println("fuzz cleanup failed: " + cleanup);
        } catch (IOException ignored) {
        }
    }

    /** One batch's cases, run side by side. */
    static class Batch {
        final String name;
        final List<JsonObject> cases = new ArrayList<>();
        final List<BlockPos> origins = new ArrayList<>();
        final List<ServerPlayer> actors = new ArrayList<>();
        final JsonArray results = new JsonArray();
        final List<JsonObject> traces = new ArrayList<>();
        final List<JsonArray> fluidRegions=new ArrayList<>();
        int tick;
        int activeCase;
        int settling = 2;
        boolean coverageCaptured;
        boolean yielded;
        final Path outputDirectory;

        Batch(String name, JsonArray in, ServerLevel level, Path directory) {
            this.name = name;
            outputDirectory = directory.resolve("outbox");
            for (int i = 0; i < in.size(); i++) {
                cases.add(in.get(i).getAsJsonObject());
                int version =
                        cases.get(i).has("version") ? cases.get(i).get("version").getAsInt() : 1;
                if (version != 1 && version != 3)
                    throw new IllegalArgumentException("unsupported scenario version");
                if (i >= SLOTS)
                    throw new IllegalArgumentException("a batch holds at most " + SLOTS + " cases");
                origins.add(slot(i));
                actors.add(null);
                results.add(new JsonObject());
                traces.add(new JsonObject());
                fluidRegions.add(FluidBounds.enabled(cases.get(i))?FluidBounds.region(cases.get(i)):new JsonArray());
            }
        }

        void start(ServerLevel level) {
            CaseIsolation.empty(level);
            if (name.startsWith("cover-") && cases.size() != 1)
                throw new IllegalArgumentException("coverage requests contain one case");
        }

        /** One tick of the active case; disposal settles before the next namespace starts. */
        boolean step(ServerLevel level) {
            CaseIsolation.settings(level);
            if (settling > 0) {
                if (--settling == 0) {
                    CaseIsolation.begin(level);
                    if (activeCase == cases.size()) return true;
                    actors.set(activeCase, PlayerActor.create(level, activeCase));
                    var next = cases.get(activeCase);
                    level.setDayTime(next.has("day_time") ? next.get("day_time").getAsLong() : 0);
                    if (name.startsWith("cover-")) coverage(null);
                }
                return false;
            }
            if (tick == 0 && !cases.isEmpty() && cases.get(activeCase).has("_start_phase")) {
                int phase = cases.get(activeCase).get("_start_phase").getAsInt();
                if (phase < 0 || phase >= 10) throw new IllegalArgumentException("start phase 0..9");
                if (Math.floorMod(level.getGameTime(), 10) != phase) return false;
                // Waiting happens before inputs. Preserve the ordinary start's one elapsed
                // level tick so clocks see the same scenario day time in every context.
                long dayTime = cases.get(activeCase).has("day_time") ? cases.get(activeCase).get("day_time").getAsLong() : 0;
                level.setDayTime(dayTime + 1);
            }
            if(tick==0) cases.get(activeCase).addProperty("_save_phase",Math.floorMod(level.getGameTime(),10));
            for (int i = activeCase; i == activeCase; i++) {
                JsonObject c = cases.get(i);
                CaseIsolation.disposeUnobservedDrops(level,origins.get(i));
                FluidBounds.check(level,origins.get(i),fluidRegions.get(i));
                if (c.has("checkpoints"))
                    for (JsonElement t : c.getAsJsonArray("checkpoints"))
                        if (t.getAsInt() == tick)
                            traces.get(i)
                                    .add(
                                            Integer.toString(tick),
                                            Observations.snapshot(level, origins.get(i), c, actors.get(i)));
                if (tick == c.get("ticks").getAsInt()) {
                    JsonObject result = Observations.snapshot(level, origins.get(i), c, actors.get(i));
                    result.add("checkpoints", traces.get(i));
                    results.set(i, result);
                    if (name.startsWith("cover-")) {
                        coverage(outputDirectory.resolve(name + ".exec"));
                        coverageCaptured = true;
                    }
                    CaseIsolation.assertContained(level, origins.get(i));
                    CaseIsolation.clear(level, origins.get(i));
                    PlayerActor.release(level, actors.set(i, null));
                    activeCase++;
                    yielded = false;
                    tick = 0;
                    settling = 2;
                    return false;
                }
                actors.get(i).getCooldowns().tick();
                // A player places the blocks, one each tick.
                JsonArray blocks = c.getAsJsonArray("blocks");
                if (tick < blocks.size())
                    place(level, origins.get(i), blocks.get(tick).getAsJsonObject());
                // A player sets a motor's speed after placing it, on the next tick.
                if (tick >= 1 && tick - 1 < blocks.size())
                    setMotor(level, actors.get(i), origins.get(i), blocks.get(tick - 1).getAsJsonObject());
                if (!c.has("events")) continue;
                for (JsonElement ev : c.getAsJsonArray("events")) {
                    JsonObject o = ev.getAsJsonObject();
                    // A motor placed the tick before gets its speed now.
                    if (o.get("tick").getAsInt() + 1 == tick && o.has("state"))
                        setMotor(level, actors.get(i), origins.get(i), o);
                }
                for (JsonElement ev : c.getAsJsonArray("events")) {
                    JsonObject o = ev.getAsJsonObject();
                    if (o.get("tick").getAsInt() != tick) continue;
                    if (o.has("action") && o.getAsJsonObject("action").get("kind").getAsString().equals("reload"))
                        Persistence.reload(level, origins.get(i), c);
                    else if (o.has("action"))
                        Actions.apply(
                                level,
                                actors.get(i),
                                at(origins.get(i), o),
                                origins.get(i), o.getAsJsonObject("action"));
                    else if (o.has("state")) place(level, origins.get(i), o);
                    // HandCrankBlock.useWithoutItem: turn(sneaking).
                    else if (o.has("turn")
                            && level.getBlockEntity(at(origins.get(i), o))
                                    instanceof HandCrankBlockEntity h)
                        h.turn(o.get("turn").getAsBoolean());
                }
            }
            // What is placed here acts from the next level tick: a case's tick t is level tick
            // start + t + 1,
            // so it is read here once its last tick, `ticks`, has run.

            tick++;
            return false;
        }

        void clear(ServerLevel level) {
            for (BlockPos origin : origins) CaseIsolation.clear(level, origin);
        }

        void releaseActors(ServerLevel level) {
            for (var actor : actors) if (actor != null) PlayerActor.release(level, actor);
            actors.replaceAll(ignored -> null);
        }

        static BlockPos at(BlockPos origin, JsonObject b) {
            JsonArray p = b.getAsJsonArray("pos");
            return origin.offset(p.get(0).getAsInt(), p.get(1).getAsInt(), p.get(2).getAsInt());
        }

        static void place(ServerLevel level, BlockPos origin, JsonObject b) {
            BlockPos pos = at(origin, b);
            BlockState state;
            try {
                state =
                        BlockStateParser.parseForBlock(
                                        BuiltInRegistries.BLOCK.asLookup(),
                                        b.get("state").getAsString(),
                                        false)
                                .blockState();
            } catch (Exception ex) {
                throw new RuntimeException("bad state " + b.get("state"), ex);
            }
            level.setBlock(pos, state, 3);
        }

        static void setMotor(ServerLevel level, ServerPlayer player, BlockPos origin, JsonObject b) {
            if (!b.has("motor")
                    || !(level.getBlockEntity(at(origin, b)) instanceof CreativeMotorBlockEntity m))
                return;
            try {
                BlockState expected =
                        BlockStateParser.parseForBlock(
                                        BuiltInRegistries.BLOCK.asLookup(),
                                        b.get("state").getAsString(),
                                        false)
                                .blockState();
                if (level.getBlockState(at(origin, b)) == expected)
                    Actions.speedSetting(level, player, at(origin, b),
                            b.get("motor").getAsInt() < 0 ? 0 : 1, Math.abs(b.get("motor").getAsInt()));
            } catch (Exception ex) {
                throw new IllegalArgumentException("bad motor placement state", ex);
            }
        }
    }
}
