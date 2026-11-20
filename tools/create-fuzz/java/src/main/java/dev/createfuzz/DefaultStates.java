package dev.createfuzz;

import com.google.gson.GsonBuilder;
import com.google.gson.JsonObject;

import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.state.properties.Property;

import java.util.TreeMap;

/**
 * Every Create block's default state, as the running binary has it: for create-horizon's
 * create-data (its `default_states.json`). Property and value names are their serialized names.
 */
final class DefaultStates {
    static String json() {
        var blocks = new TreeMap<String, TreeMap<String, String>>();
        for (var block : BuiltInRegistries.BLOCK) {
            var id = BuiltInRegistries.BLOCK.getKey(block);
            if (!id.getNamespace().equals("create")) continue;
            var properties = new TreeMap<String, String>();
            var state = block.defaultBlockState();
            for (Property<?> property : state.getProperties())
                properties.put(property.getName(), name(property, state));
            blocks.put(id.getPath(), properties);
        }
        var out = new JsonObject();
        blocks.forEach(
                (block, properties) -> {
                    var o = new JsonObject();
                    properties.forEach(o::addProperty);
                    out.add(block, o);
                });
        return new GsonBuilder().setPrettyPrinting().disableHtmlEscaping().create().toJson(out)
                + "\n";
    }

    private static <T extends Comparable<T>> String name(
            Property<T> property, net.minecraft.world.level.block.state.BlockState state) {
        return property.getName(state.getValue(property));
    }
}
