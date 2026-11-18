package com.simibubi.create.content.kinetics;

import java.util.HashMap;
import java.util.Map;
import com.simibubi.create.content.kinetics.base.KineticBlockEntity;

public final class NetworkOracle {
    public static void main(String[] args) {
        float speed = Float.parseFloat(args[0]);
        float capacity = Float.parseFloat(args[1]);
        int presses = Integer.parseInt(args[2]);
        KineticNetwork network = new KineticNetwork();
        network.add(new KineticBlockEntity(true, speed, capacity, 0));
        for (int i = 0; i < presses; i++)
            network.add(new KineticBlockEntity(false, speed, 0, 8));
        network.updateNetwork();
        System.out.println("{\"capacity\":" + network.calculateCapacity()
            + ",\"stress\":" + network.calculateStress() + "}");
    }
}

// Only needed for KineticNetwork.remove's cleanup reference; the fixture never removes.
final class TorquePropagator {
    static final Map<Object, Map<Long, KineticNetwork>> networks = new HashMap<>();
}
