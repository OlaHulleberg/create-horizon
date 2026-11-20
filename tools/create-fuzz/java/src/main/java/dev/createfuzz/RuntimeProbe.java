package dev.createfuzz;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;

import net.minecraft.core.Direction;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.Mth;

/** Binary-only measurement: never consult Minecraft implementation sources. */
final class RuntimeProbe {
    static JsonArray angles(ServerLevel level) {
        var player = PlayerActor.create(level, -2);
        JsonArray result = new JsonArray();
        for (float yaw : new float[] {0, 45, 90, 135, 180, 225, 270, 315}) {
            player.setYRot(yaw);
            player.setYHeadRot(yaw);
            player.setXRot(0);
            float radians = -yaw * ((float) Math.PI / 180F);
            JsonObject sample = new JsonObject();
            sample.addProperty("yaw", yaw);
            sample.addProperty("radians", radians);
            sample.addProperty("sin", Mth.sin(radians));
            sample.addProperty("cos", Mth.cos(radians));
            sample.addProperty(
                    "sin_bits",
                    Integer.toUnsignedString(Float.floatToRawIntBits(Mth.sin(radians))));
            sample.addProperty(
                    "cos_bits",
                    Integer.toUnsignedString(Float.floatToRawIntBits(Mth.cos(radians))));
            sample.add("nearest", CreateFuzz.GSON.toJsonTree(Direction.orderedByNearest(player)));
            result.add(sample);
        }
        return result;
    }
}
