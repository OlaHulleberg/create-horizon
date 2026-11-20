package dev.createfuzz;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.simibubi.create.content.kinetics.base.IRotate;

import net.minecraft.core.registries.BuiltInRegistries;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashSet;

/**
 * Checks the MIT source-derived contract against the loaded binary, without consulting host
 * sources.
 */
final class Catalogue {
    static void audit() throws java.io.IOException {
        var fluids = JsonParser.parseString(Files.readString(Path.of(System.getenv("FUZZ_FLUID_MANIFEST")))).getAsJsonObject();
        for (var element : fluids.getAsJsonArray("blocks")) {
            var entry = element.getAsJsonObject();
            var block = BuiltInRegistries.BLOCK.get(net.minecraft.resources.ResourceLocation.parse(entry.get("id").getAsString()));
            var properties = entry.getAsJsonObject("properties");
            if (properties.size() != block.getStateDefinition().getProperties().size()) throw new IllegalStateException("fluid properties differ");
            for (var property : block.getStateDefinition().getProperties()) {
                var expected = new HashSet<String>();
                for (var value : properties.getAsJsonArray(property.getName())) expected.add(value.getAsString());
                var actual = new HashSet<String>();
                for (var value : property.getPossibleValues()) actual.add(valueName(property,value));
                if (!expected.equals(actual)) throw new IllegalStateException("fluid domain differs: " + property.getName());
            }
        }
        var entries =
                JsonParser.parseString(Files.readString(Path.of(System.getenv("FUZZ_MANIFEST"))))
                        .getAsJsonArray();
        var ids = new HashSet<String>();
        for (JsonElement element : entries) {
            JsonObject entry = element.getAsJsonObject();
            String id = entry.get("id").getAsString();
            if (!ids.add(id)) throw new IllegalStateException("duplicate manifest id " + id);
            var block =
                    BuiltInRegistries.BLOCK.get(net.minecraft.resources.ResourceLocation.parse(id));
            if (!(block instanceof IRotate))
                throw new IllegalStateException("manifest id is not kinetic: " + id);
            auditConnections(entry, block);
            var properties = entry.getAsJsonObject("properties");
            if (properties.size() != block.getStateDefinition().getProperties().size())
                throw new IllegalStateException("manifest properties differ: " + id);
            for (var property : block.getStateDefinition().getProperties()) {
                var values = new HashSet<String>();
                for (JsonElement v : properties.getAsJsonArray(property.getName()))
                    values.add(v.getAsString());
                var actual = new HashSet<String>();
                for (var v : property.getPossibleValues()) actual.add(valueName(property, v));
                if (!values.equals(actual))
                    throw new IllegalStateException(
                            "manifest domain differs: "
                                    + id
                                    + " "
                                    + property.getName()
                                    + " "
                                    + actual);
            }
        }
        for (var block : BuiltInRegistries.BLOCK) {
            var id = BuiltInRegistries.BLOCK.getKey(block);
            if (id.getNamespace().equals("create")
                    && block instanceof IRotate
                    && !ids.contains(id.toString()))
                throw new IllegalStateException("unrepresented kinetic block: " + id);
        }
    }

    private static void auditConnections(
            JsonObject entry, net.minecraft.world.level.block.Block block) {
        if (!entry.get("excluded").isJsonNull()) return;
        var rule = entry.getAsJsonObject("connection");
        for (var state : block.getStateDefinition().getPossibleStates()) {
            var props = new java.util.HashMap<String, String>();
            for (var p : state.getProperties())
                props.put(p.getName(), valueName(p, state.getValue(p)));
            var f = net.minecraft.core.Direction.byName(props.getOrDefault("facing", "up"));
            var a =
                    switch (rule.get("axis").getAsString()) {
                        case "property" ->
                                net.minecraft.core.Direction.Axis.valueOf(
                                        props.get("axis").toUpperCase(java.util.Locale.ROOT));
                        case "facing" -> f.getAxis();
                        case "y" -> net.minecraft.core.Direction.Axis.Y;
                        case "clockwise" -> f.getClockWise().getAxis();
                        case "directional", "saw" -> {
                            if (rule.get("axis").getAsString().equals("saw")
                                    && f.getAxis() != net.minecraft.core.Direction.Axis.Y)
                                yield f.getAxis();
                            var other =
                                    new java.util.ArrayList<net.minecraft.core.Direction.Axis>();
                            for (var axis : net.minecraft.core.Direction.Axis.values())
                                if (axis != f.getAxis()) other.add(axis);
                            yield other.get(
                                    Boolean.parseBoolean(props.get("axis_along_first")) ? 0 : 1);
                        }
                        default ->
                                throw new IllegalStateException("unsupported generated axis rule");
                    };
            var rotate = (IRotate) block;
            if (rotate.getRotationAxis(state) != a)
                throw new IllegalStateException("rotation axis metadata differs: " + state);
            for (var d : net.minecraft.core.Direction.values()) {
                boolean expected =
                        switch (rule.get("shafts").getAsString()) {
                            case "axis" -> d.getAxis() == a;
                            case "off_axis" -> d.getAxis() != a;
                            case "front" -> d == f;
                            case "back" -> d == f.getOpposite();
                            case "down" -> d == net.minecraft.core.Direction.DOWN;
                            case "up" -> d == net.minecraft.core.Direction.UP;
                            case "clockwise" -> d == f.getClockWise();
                            case "none" -> false;
                            case "flags" ->
                                    d.getAxis() == a
                                            && Boolean.parseBoolean(
                                                    props.get(
                                                            d.getAxisDirection()
                                                                            == net.minecraft.core
                                                                                    .Direction
                                                                                    .AxisDirection
                                                                                    .POSITIVE
                                                                    ? "top_shaft"
                                                                    : "bottom_shaft"));
                            case "saw" ->
                                    f.getAxis() == net.minecraft.core.Direction.Axis.Y
                                            ? d.getAxis() == a
                                            : d == f.getOpposite();
                            default -> throw new IllegalStateException("unsupported shaft rule");
                        };
                if (rotate.hasShaftTowards(null, net.minecraft.core.BlockPos.ZERO, state, d)
                        != expected)
                    throw new IllegalStateException("shaft metadata differs: " + state + " " + d);
            }
            String cog = rule.get("cog").isJsonNull() ? "none" : rule.get("cog").getAsString();
            if (com.simibubi.create.content.kinetics.simpleRelays.ICogWheel.isSmallCog(state)
                            != cog.equals("small")
                    || com.simibubi.create.content.kinetics.simpleRelays.ICogWheel.isLargeCog(state)
                            != cog.equals("large"))
                throw new IllegalStateException("cog metadata differs: " + state);
        }
    }

    @SuppressWarnings({"rawtypes", "unchecked"})
    private static String valueName(
            net.minecraft.world.level.block.state.properties.Property property, Comparable value) {
        return property.getName(value);
    }
}
