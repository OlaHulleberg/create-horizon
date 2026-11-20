package dev.createfuzz;

import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import com.simibubi.create.content.kinetics.base.KineticBlockEntity;
import net.minecraft.core.BlockPos;
import net.minecraft.nbt.NbtUtils;

/** Read the actual server save, without runtime network ids or absolute world coordinates. */
final class Persistence {
    /** Fresh block entities, retaining blocks and scheduled world ticks, at the action boundary. */
    static void reload(net.minecraft.server.level.ServerLevel level, BlockPos origin, JsonObject scenario) {
        var lo = new int[]{24,24,24};
        var hi = new int[]{-24,-24,-24};
        var entries = new java.util.ArrayList<com.google.gson.JsonElement>();
        scenario.getAsJsonArray("blocks").forEach(entries::add);
        if (scenario.has("events")) scenario.getAsJsonArray("events").forEach(entries::add);
        for (var e : entries) for (int a=0;a<3;a++) {
            int v=e.getAsJsonObject().getAsJsonArray("pos").get(a).getAsInt();
            lo[a]=Math.min(lo[a],v); hi[a]=Math.max(hi[a],v);
        }
        var tags = new java.util.LinkedHashMap<BlockPos,net.minecraft.nbt.CompoundTag>();
        for(int x=lo[0]-1;x<=hi[0]+1;x++) for(int y=lo[1]-1;y<=hi[1]+1;y++) for(int z=lo[2]-1;z<=hi[2]+1;z++) {
            var pos=origin.offset(x,y,z);
            var be=level.getBlockEntity(pos);
            if(be==null) continue;
            var tag=be.saveWithoutMetadata(level.registryAccess());
            tag.putString("id",net.minecraft.core.registries.BuiltInRegistries.BLOCK_ENTITY_TYPE.getKey(be.getType()).toString());
            tags.put(pos,tag);
        }
        // Replacing entities must not change the world's existing ticking order.
        // Coordinate order differs from placement order (e.g. shaft, handle, shaft).
        try {
            var field = net.minecraft.world.level.Level.class.getDeclaredField("blockEntityTickers");
            field.setAccessible(true);
            var ordered = new java.util.LinkedHashMap<BlockPos, net.minecraft.nbt.CompoundTag>();
            for (var entry : (java.util.List<?>) field.get(level)) {
                var ticker = (net.minecraft.world.level.block.entity.TickingBlockEntity) entry;
                var pos = ticker.getPos();
                if (!ticker.isRemoved() && tags.containsKey(pos)) ordered.put(pos.immutable(), tags.get(pos));
            }
            ordered.putAll(tags);
            tags = ordered;
        } catch (ReflectiveOperationException e) { throw new IllegalStateException(e); }
        for(var pos:tags.keySet()) {
            var be=level.getBlockEntity(pos);
            be.onChunkUnloaded();
            level.removeBlockEntity(pos);
        }
        // Every case owns its complete kinetic world. Restored entities must not
        // rejoin maps containing removed Java objects from the previous instance.
        NetworkIsolation.reset(level);
        for(var entry:tags.entrySet()) {
            var pos=entry.getKey();
            var fresh=net.minecraft.world.level.block.entity.BlockEntity.loadStatic(pos,level.getBlockState(pos),entry.getValue(),level.registryAccess());
            if(fresh==null) throw new IllegalStateException("reload failed at "+pos);
            level.setBlockEntity(fresh);
        }
    }
    private static net.minecraft.nbt.CompoundTag beltTag(com.simibubi.create.content.kinetics.belt.BeltBlockEntity b) {
        var tag=new net.minecraft.nbt.CompoundTag();
        tag.putFloat("Speed",b.getTheoreticalSpeed());tag.putBoolean("NeedsSpeedUpdate",b.needsSpeedUpdate());
        if(b.source!=null) tag.put("Source",NbtUtils.writeBlockPos(b.source));
        if(b.network!=null) {
            var n=new net.minecraft.nbt.CompoundTag();
            n.putFloat("Stress",TransportObservations.<Float>field(b,KineticBlockEntity.class,"stress"));
            n.putFloat("Capacity",TransportObservations.<Float>field(b,KineticBlockEntity.class,"capacity"));
            n.putInt("Size",TransportObservations.<Integer>field(b,KineticBlockEntity.class,"networkSize"));
            n.putFloat("AddedStress",TransportObservations.<Float>field(b,KineticBlockEntity.class,"lastStressApplied"));
            n.putFloat("AddedCapacity",TransportObservations.<Float>field(b,KineticBlockEntity.class,"lastCapacityProvided"));tag.put("Network",n);
        }
        tag.put("Controller",NbtUtils.writeBlockPos(b.getController()));tag.putBoolean("IsController",b.isController());
        tag.putInt("Length",b.beltLength);tag.putInt("Index",b.index);tag.putString("Casing",b.casing.name());
        if(b.isController()) {
            var inv=b.getInventory();var owner=com.simibubi.create.content.kinetics.belt.transport.BeltInventory.class;
            boolean positive=TransportObservations.<Boolean>field(inv,owner,"beltMovementPositive");
            var items=new java.util.ArrayList<>(inv.getTransportedItems());
            items.addAll(TransportObservations.<java.util.List<com.simibubi.create.content.kinetics.belt.transport.TransportedItemStack>>field(inv,owner,"toInsert"));
            items.removeAll(TransportObservations.<java.util.List<com.simibubi.create.content.kinetics.belt.transport.TransportedItemStack>>field(inv,owner,"toRemove"));
            items.sort(positive ? java.util.Comparator.naturalOrder() : java.util.Comparator.reverseOrder());
            var saved=new net.minecraft.nbt.CompoundTag();var list=new net.minecraft.nbt.ListTag();
            for(var item:items) list.add(item.serializeNBT(b.getLevel().registryAccess()));
            saved.put("Items",list);saved.putBoolean("PositiveOrder",positive);tag.put("Inventory",saved);
        }
        return tag;
    }
    static JsonObject payload(net.minecraft.world.level.block.entity.BlockEntity be, BlockPos origin) {
        var data=be instanceof com.simibubi.create.content.kinetics.belt.BeltBlockEntity b ? beltTag(b) : be.saveWithoutMetadata(be.getLevel().registryAccess());
        var out=new JsonObject();
        if(be instanceof com.simibubi.create.content.kinetics.transmission.sequencer.SequencedGearshiftBlockEntity) {
            out.addProperty("kind","sequencer");
            var instructions=new com.google.gson.JsonArray();
            for(var element:data.getList("Instructions",10)) {
                var tag=(net.minecraft.nbt.CompoundTag)element;
                var instruction=new JsonObject();
                instruction.addProperty("instruction",tag.getString("Type"));
                instruction.addProperty("modifier",tag.getString("Modifier"));
                instruction.addProperty("value",tag.getInt("Value")); instructions.add(instruction);
            }
            out.add("instructions",instructions);
            out.addProperty("current",data.getInt("InstructionIndex"));
            out.addProperty("duration",data.getInt("InstructionDuration"));
            out.addProperty("progress",data.getFloat("InstructionProgress"));
            out.addProperty("timer",data.getInt("Timer"));
            out.addProperty("powered",data.getBoolean("PrevPowered"));
        } else if(be instanceof com.simibubi.create.content.kinetics.crank.ValveHandleBlockEntity) {
            out.addProperty("in_use",data.getInt("InUse"));out.addProperty("backwards",data.getBoolean("Backwards"));out.addProperty("start_angle",data.getInt("StartAngle"));
            out.addProperty("kind","valve");out.addProperty("selected",data.getInt("ScrollValue"));
            out.addProperty("total",data.getInt("TotalUseTicks"));out.addProperty("target",data.getInt("TargetAngle"));
        } else if(be instanceof com.simibubi.create.content.kinetics.crank.HandCrankBlockEntity) {
            out.addProperty("kind","crank");out.addProperty("in_use",data.getInt("InUse"));out.addProperty("backwards",data.getBoolean("Backwards"));
        } else if(be instanceof com.simibubi.create.content.kinetics.belt.BeltBlockEntity belt) {
            out.addProperty("kind","belt");
            var controller=NbtUtils.readBlockPos(data,"Controller").orElseThrow().subtract(origin);
            out.add("controller",CreateFuzz.GSON.toJsonTree(new int[]{controller.getX(),controller.getY(),controller.getZ()}));
            out.addProperty("length",data.getInt("Length"));out.addProperty("index",data.getInt("Index"));
            out.addProperty("casing",data.getString("Casing").toLowerCase(java.util.Locale.ROOT));
            out.add("inventory",JsonNull.INSTANCE);
            if(data.getBoolean("IsController")) {
                var inventory=new JsonObject();var items=new com.google.gson.JsonArray();
                var inv=data.getCompound("Inventory");
                inventory.addProperty("positive",inv.getBoolean("PositiveOrder"));
                for(var element:inv.getList("Items",10)) items.add(savedItem((net.minecraft.nbt.CompoundTag)element,be));
                inventory.add("items",items);out.add("inventory",inventory);
            }
        } else if(be instanceof com.simibubi.create.content.logistics.depot.DepotBlockEntity) {
            out.addProperty("kind","depot");out.add("held",JsonNull.INSTANCE);
            if(data.contains("HeldItem")) out.add("held",savedItem(data.getCompound("HeldItem"),be).get("stack"));
        } else return null;
        return out;
    }
    private static JsonObject savedItem(net.minecraft.nbt.CompoundTag data, net.minecraft.world.level.block.entity.BlockEntity be) {
        var out=new JsonObject();
        var stack=ItemInputs.stack(net.minecraft.world.item.ItemStack.parseOptional(be.getLevel().registryAccess(),data.getCompound("Item")));
        // A stack a funnel emptied is saved until the next tick's loop top removes it.
        if(stack.isJsonNull()) {var empty=new JsonObject();empty.addProperty("item","minecraft:air");empty.addProperty("count",0);stack=empty;}
        out.add("stack",stack);
        out.addProperty("position",data.getFloat("Pos"));out.addProperty("locked",data.getBoolean("Locked"));return out;
    }
    static JsonObject saved(KineticBlockEntity be, BlockPos origin) {
        var data = be instanceof com.simibubi.create.content.kinetics.belt.BeltBlockEntity b ? beltTag(b) : be.saveWithoutMetadata(be.getLevel().registryAccess());
        var out = new JsonObject();
        out.addProperty("speed", data.getFloat("Speed"));
        out.addProperty("needs_speed_update", data.getBoolean("NeedsSpeedUpdate"));
        out.add("source", JsonNull.INSTANCE);
        if (data.contains("Source")) {
            var source = NbtUtils.readBlockPos(data, "Source").orElseThrow().subtract(origin);
            out.add("source", CreateFuzz.GSON.toJsonTree(new int[]{source.getX(),source.getY(),source.getZ()}));
        }
        out.add("network", JsonNull.INSTANCE);
        if (data.contains("Network")) {
            var n = data.getCompound("Network");
            var network = new JsonObject();
            network.addProperty("stress", n.getFloat("Stress"));
            network.addProperty("capacity", n.getFloat("Capacity"));
            network.addProperty("size", n.getInt("Size"));
            network.addProperty("added_stress", n.getFloat("AddedStress"));
            network.addProperty("added_capacity", n.getFloat("AddedCapacity"));
            out.add("network", network);
        }
        return out;
    }
}
