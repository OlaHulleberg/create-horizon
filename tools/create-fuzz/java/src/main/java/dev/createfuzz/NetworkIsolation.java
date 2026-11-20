package dev.createfuzz;

import com.simibubi.create.Create;
import net.minecraft.server.level.ServerLevel;

/** A batch owns a fresh network namespace, just as a fresh level does. */
final class NetworkIsolation {
    static void reset(ServerLevel level) {
        Create.TORQUE_PROPAGATOR.onUnloadWorld(level);
        Create.TORQUE_PROPAGATOR.onLoadWorld(level);
    }
}
