package com.simibubi.create.content.kinetics.base;

/** Minimal world/BE adapter for the unmodified upstream KineticNetwork class.
 * It models loaded identity and numeric getters only, not Minecraft gameplay. */
public final class KineticBlockEntity {
    public boolean networkDirty;
    private final boolean source;
    private final float speed, capacity, impact;
    private final Level level = new Level();
    public KineticBlockEntity(boolean source, float speed, float capacity, float impact) {
        this.source = source; this.speed = speed; this.capacity = capacity; this.impact = impact;
    }
    public boolean isSource() { return source; }
    public float getGeneratedSpeed() { return source ? speed : 0; }
    public float getTheoreticalSpeed() { return speed; }
    public float calculateAddedStressCapacity() { return capacity; }
    public float calculateStressApplied() { return impact; }
    public Level getLevel() { return level; }
    public Object getBlockPos() { return this; }
    public void updateFromNetwork(float capacity, float stress, int size) {}
    public static final class Level {
        public Object getBlockEntity(Object pos) { return pos; }
    }
}
