package dev.createfuzz;

import com.google.gson.*;
import com.simibubi.create.content.kinetics.belt.BeltBlockEntity;
import com.simibubi.create.content.kinetics.belt.transport.*;
import com.simibubi.create.content.logistics.depot.DepotBehaviour;
import com.simibubi.create.foundation.blockEntity.behaviour.BlockEntityBehaviour;
import net.minecraft.world.level.block.entity.BlockEntity;
import java.util.*;

/** Read Create's live semantic state, including pending queues, without write()'s mutation. */
final class TransportObservations {
    static com.google.gson.JsonArray directions(java.util.Collection<net.minecraft.core.Direction> dirs) {
        var names=new java.util.TreeSet<String>();for(var d:dirs) names.add(d.getSerializedName());
        var out=new com.google.gson.JsonArray();for(var n:names) out.add(n);return out;
    }
    /** BeltTunnelBlockEntity.flaps keys and sides, as updateTunnelConnections left them. */
    static JsonObject tunnel(BlockEntity be) {
        if(!(be instanceof com.simibubi.create.content.logistics.tunnel.BeltTunnelBlockEntity t)) return null;
        JsonObject o=new JsonObject();o.add("flaps",directions(t.flaps.keySet()));o.add("sides",directions(t.sides));
        if(t instanceof com.simibubi.create.content.logistics.tunnel.BrassTunnelBlockEntity b) o.add("brass",brass(b));
        return o;
    }
    /** BrassTunnelBlockEntity's distribution state: the stack it holds and the side it came in by,
     * distributionProgress, previousOutputIndex, syncedOutputActive, connectedLeft/Right, the
     * selection mode and the sided filters. */
    static JsonObject brass(com.simibubi.create.content.logistics.tunnel.BrassTunnelBlockEntity b) {
        var owner=com.simibubi.create.content.logistics.tunnel.BrassTunnelBlockEntity.class;
        JsonObject o=new JsonObject();
        o.add("stack",ItemInputs.stack(b.getStackToDistribute()));
        net.minecraft.core.Direction from=field(b,owner,"stackEnteredFrom");
        o.add("entered_from",from==null?JsonNull.INSTANCE:new JsonPrimitive(from.getSerializedName()));
        // tick reads distributionProgress only while it holds a stack or hands out with its
        // group (a tunnel that never held one starts at 0, a later empty one at -1).
        boolean synced=field(b,owner,"syncedOutputActive");
        if(b.getStackToDistribute().isEmpty() && !synced) o.add("progress",JsonNull.INSTANCE);
        else o.addProperty("progress",(float)field(b,owner,"distributionProgress"));
        o.addProperty("previous",(int)field(b,owner,"previousOutputIndex"));
        o.addProperty("synced",(boolean)field(b,owner,"syncedOutputActive"));
        o.addProperty("connected_left",b.isConnected(true));o.addProperty("connected_right",b.isConnected(false));
        com.simibubi.create.foundation.blockEntity.behaviour.scrollValue.ScrollOptionBehaviour<?> mode=field(b,owner,"selectionMode");
        o.addProperty("mode",mode.get().name().toLowerCase(java.util.Locale.ROOT));
        com.simibubi.create.foundation.blockEntity.behaviour.filtering.SidedFilteringBehaviour filtering=field(b,owner,"filtering");
        JsonObject filters=new JsonObject();
        if(filtering!=null) for(var d:net.minecraft.core.Direction.values()) {
            var f=filtering.get(d);if(f==null||f.getFilter().isEmpty()) continue;
            filters.addProperty(d.getSerializedName(),net.minecraft.core.registries.BuiltInRegistries.ITEM.getKey(f.getFilter().getItem()).toString());
        }
        o.add("filters",filters);return o;
    }
    /** FunnelBlockEntity.extractionCooldown, read without write()'s side effects. */
    static JsonObject funnel(BlockEntity be) {
        if(!(be instanceof com.simibubi.create.content.logistics.funnel.FunnelBlockEntity f)) return null;
        JsonObject o=new JsonObject();o.addProperty("cooldown",(int)field(f,com.simibubi.create.content.logistics.funnel.FunnelBlockEntity.class,"extractionCooldown"));
        // A brass funnel's filter (its item id) and amount board: its count as far as the filter's
        // stack size takes it (a count above it moves no more), "up to" or exactly.
        var filtering=BlockEntityBehaviour.get(f,com.simibubi.create.foundation.blockEntity.behaviour.filtering.FilteringBehaviour.TYPE);
        if(filtering!=null && filtering.isActive()) {
            var filter=filtering.getFilter();
            o.add("filter",filter.isEmpty()?JsonNull.INSTANCE:new JsonPrimitive(net.minecraft.core.registries.BuiltInRegistries.ITEM.getKey(filter.getItem()).toString()));
            o.addProperty("amount",Math.min(filtering.count,filtering.getMaxStackSize()));o.addProperty("up_to",filtering.upTo);
        }
        return o;
    }
    /** ChuteBlockEntity.item and its position along the chute. */
    static JsonObject chute(BlockEntity be) {
        if(!(be instanceof com.simibubi.create.content.logistics.chute.ChuteBlockEntity c)) return null;
        JsonObject o=new JsonObject();o.add("item",ItemInputs.stack(c.getItem()));
        net.createmod.catnip.animation.LerpedFloat position=field(c,com.simibubi.create.content.logistics.chute.ChuteBlockEntity.class,"itemPosition");
        // The position is only meaningful while it holds a stack.
        o.addProperty("position",c.getItem().isEmpty()?0f:position.getValue());return o;
    }
    /** EjectorBlockEntity: its state, power, plate (lidProgress), launcher distances, stack size,
     * the stacks in flight (ticks flown, stack) and where an obstacle stops them. */
    static JsonObject ejector(BlockEntity be) {
        if(!(be instanceof com.simibubi.create.content.logistics.depot.EjectorBlockEntity e)) return null;
        var owner=com.simibubi.create.content.logistics.depot.EjectorBlockEntity.class;
        JsonObject o=new JsonObject();
        Enum<?> state=field(e,owner,"state");o.addProperty("state",state.name().toLowerCase(java.util.Locale.ROOT));
        o.addProperty("powered",(boolean)field(e,owner,"powered"));
        net.createmod.catnip.animation.LerpedFloat lid=field(e,owner,"lidProgress");o.addProperty("lid",lid.getValue());
        com.simibubi.create.content.logistics.depot.EntityLauncher launcher=field(e,owner,"launcher");
        o.addProperty("h",launcher.getHorizontalDistance());o.addProperty("v",launcher.getVerticalDistance());
        com.simibubi.create.foundation.blockEntity.behaviour.scrollValue.ScrollValueBehaviour max=field(e,owner,"maxStackSize");
        o.addProperty("max_stack",max.getValue());
        JsonArray flying=new JsonArray();
        java.util.List<net.createmod.catnip.data.IntAttached<net.minecraft.world.item.ItemStack>> launched=field(e,owner,"launchedItems");
        for(var f:launched) {JsonObject i=new JsonObject();i.addProperty("time",f.getFirst());i.add("stack",ItemInputs.stack(f.getSecond()));flying.add(i);}
        o.add("flying",flying);
        Object early=field(e,owner,"earlyTarget");
        o.add("early_time",early==null?JsonNull.INSTANCE:new JsonPrimitive((float)field(e,owner,"earlyTargetTime")));
        return o;
    }
    @SuppressWarnings("unchecked")
    static <T> T field(Object o,Class<?> owner,String name) {
        try {var f=owner.getDeclaredField(name);f.setAccessible(true);return (T)f.get(o);}
        catch(ReflectiveOperationException e) {throw new IllegalStateException(e);}
    }
    static JsonObject item(TransportedItemStack s,boolean pending,Boolean stalled) {
        // A stack a funnel or crusher emptied stays in the list until the next tick's loop top.
        JsonElement stack=ItemInputs.stack(s.stack);
        if(stack.isJsonNull()) {JsonObject empty=new JsonObject();empty.addProperty("item","minecraft:air");empty.addProperty("count",0);stack=empty;}
        JsonObject o=new JsonObject();o.add("stack",stack);
        o.addProperty("position",s.beltPosition);o.addProperty("locked",s.locked);o.addProperty("pending",pending);
        o.add("stalled",stalled==null?JsonNull.INSTANCE:new JsonPrimitive(stalled));return o;
    }
    static JsonObject observe(BlockEntity be) {
        JsonObject out=new JsonObject();var items=new ArrayList<JsonObject>();var outputs=new ArrayList<JsonObject>();
        if(be instanceof BeltBlockEntity b) {
            if(!b.isController()) return null;
            out.addProperty("kind","belt");var inv=b.getInventory();
            List<TransportedItemStack> removed=field(inv,BeltInventory.class,"toRemove");
            for(var s:inv.getTransportedItems()) if(!removed.contains(s))
                items.add(item(s,false,b.getSpeed()==0 || s.beltPosition==s.prevBeltPosition));
            List<TransportedItemStack> pending=field(inv,BeltInventory.class,"toInsert");
            for(var s:pending) items.add(item(s,true,null));
        } else {
            if(!(be instanceof com.simibubi.create.content.logistics.depot.DepotBlockEntity)
                && !(be instanceof com.simibubi.create.content.logistics.depot.EjectorBlockEntity)) return null;
            var depot=BlockEntityBehaviour.get(be,DepotBehaviour.TYPE);
            if(depot==null) return null;
            out.addProperty("kind","depot");
            TransportedItemStack held=field(depot,DepotBehaviour.class,"heldItem");
            if(held!=null) items.add(item(held,false,null));
            // An ejector's merging stacks slide in (`incoming`) before they join the held one.
            List<TransportedItemStack> incoming=field(depot,DepotBehaviour.class,"incoming");
            for(var s:incoming) items.add(item(s,true,null));
            // SharedDepotBlockMethods.getComparatorInputOverride.
            out.addProperty("comparator",be.getBlockState().getAnalogOutputSignal(be.getLevel(),be.getBlockPos()));
            net.neoforged.neoforge.items.IItemHandler buffer=field(depot,DepotBehaviour.class,"processingOutputBuffer");
            for(int i=0;i<buffer.getSlots();i++)if(!buffer.getStackInSlot(i).isEmpty())outputs.add(ItemInputs.stack(buffer.getStackInSlot(i)).getAsJsonObject());
        }
        items.sort(Comparator.comparingDouble((JsonObject i)->i.get("position").getAsDouble())
            .thenComparing(i->i.getAsJsonObject("stack").get("item").getAsString())
            .thenComparingInt(i->i.getAsJsonObject("stack").get("count").getAsInt()));
        outputs.sort(Comparator.comparing((JsonObject s)->s.get("item").getAsString()).thenComparingInt(s->s.get("count").getAsInt()));
        out.add("outputs",CreateFuzz.GSON.toJsonTree(outputs));out.add("items",CreateFuzz.GSON.toJsonTree(items));return out;
    }
}
