package dev.createfuzz;

import com.google.gson.*;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.level.*;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.phys.*;
import java.util.*;

/** Player use, or a player-dropped entity delivered at the controlled landing boundary.
 * Never inserts into a Create inventory directly. Native item flight is outside this input. */
final class ItemInputs {
    static void audit() {
        try {
            var doc=JsonParser.parseString(java.nio.file.Files.readString(java.nio.file.Path.of(System.getenv("FUZZ_TRANSPORT_MANIFEST")))).getAsJsonObject();
            for(var e:doc.getAsJsonArray("items")) {
                var i=e.getAsJsonObject();var id=ResourceLocation.parse(i.get("id").getAsString());
                if(!BuiltInRegistries.ITEM.containsKey(id) || new ItemStack(BuiltInRegistries.ITEM.get(id)).getMaxStackSize()!=i.get("max_stack").getAsInt()) throw new IllegalStateException("transport item contract: "+id);
            }
            for(var e:doc.getAsJsonArray("blocks")) {
                var b=e.getAsJsonObject();var id=ResourceLocation.parse(b.get("id").getAsString());
                if(!BuiltInRegistries.BLOCK.containsKey(id)) throw new IllegalStateException("transport block missing: "+id);
                var state=BuiltInRegistries.BLOCK.get(id).defaultBlockState();
                for(var prop:b.getAsJsonObject("properties").entrySet()) {
                    var property=state.getBlock().getStateDefinition().getProperty(prop.getKey());
                    if(property==null) throw new IllegalStateException("transport property missing");
                    for(var value:prop.getValue().getAsJsonArray()) if(property.getValue(value.getAsString()).isEmpty()) throw new IllegalStateException("transport value missing");
                }
            }
        } catch(java.io.IOException e) {throw new IllegalStateException(e);}
    }
    private static final Map<ServerPlayer,JsonArray> outcomes=new WeakHashMap<>();
    static JsonArray outcomes(ServerPlayer p) {return outcomes.getOrDefault(p,new JsonArray()).deepCopy();}
    static ItemStack stack(JsonElement e) {
        if(e==null || e.isJsonNull()) return ItemStack.EMPTY;
        JsonObject s=e.getAsJsonObject();
        var item=BuiltInRegistries.ITEM.get(ResourceLocation.parse(s.get("item").getAsString()));
        int count=s.get("count").getAsInt();
        ItemStack stack=new ItemStack(item,count);
        if(count<1 || count>stack.getMaxStackSize()) throw new IllegalArgumentException("invalid item input");
        return stack;
    }
    static JsonElement stack(ItemStack s) {
        if(s.isEmpty()) return JsonNull.INSTANCE;
        JsonObject o=new JsonObject();o.addProperty("item",BuiltInRegistries.ITEM.getKey(s.getItem()).toString());o.addProperty("count",s.getCount());return o;
    }
    static Map<String,Integer> inventory(ServerPlayer p) {
        var totals=new TreeMap<String,Integer>();
        for(int i=0;i<p.getInventory().getContainerSize();i++) {
            ItemStack s=p.getInventory().getItem(i);
            if(!s.isEmpty()) totals.merge(BuiltInRegistries.ITEM.getKey(s.getItem()).toString(),s.getCount(),Integer::sum);
        }
        return totals;
    }
    /** Another block's item access (a hopper's, a pipe's): the block's ItemHandler capability on
     * `side`, one insertItem or extractItem call on `slot`, simulated or not. Not a player input. */
    static void capability(ServerLevel level,ServerPlayer p,BlockPos pos,BlockPos origin,JsonObject a) {
        Direction side=a.get("side")==null||a.get("side").isJsonNull()?null:Direction.byName(a.get("side").getAsString());
        int slot=a.get("slot").getAsInt();boolean simulate=a.get("simulate").getAsBoolean();
        ItemStack offered=stack(a.get("insert"));ItemStack remainder=offered.copy();
        int extract=a.get("extract")==null||a.get("extract").isJsonNull()?0:a.get("extract").getAsInt();
        var handler=level.getCapability(net.neoforged.neoforge.capabilities.Capabilities.ItemHandler.BLOCK,pos,side);
        JsonArray taken=new JsonArray();
        if(handler!=null && slot>=0 && slot<handler.getSlots()) {
            if(!offered.isEmpty()) remainder=handler.insertItem(slot,offered.copy(),simulate).copy();
            else if(extract>0) {var out=handler.extractItem(slot,extract,simulate);if(!out.isEmpty()) taken.add(stack(out));}
        }
        JsonObject out=new JsonObject();BlockPos rel=pos.subtract(origin);
        out.add("pos",CreateFuzz.GSON.toJsonTree(new int[]{rel.getX(),rel.getY(),rel.getZ()}));
        out.add("offered",stack(offered));out.add("remainder",offered.isEmpty()?JsonNull.INSTANCE:stack(remainder));
        out.add("taken",taken);out.addProperty("handler",handler!=null);
        outcomes.computeIfAbsent(p,k->new JsonArray()).add(out);
    }
    static void apply(ServerLevel level,ServerPlayer p,BlockPos pos,BlockPos origin,JsonObject input) {
        ItemStack offered=stack(input.get("stack"));
        ItemStack saved=offered.copy();
        p.setItemInHand(InteractionHand.MAIN_HAND,offered);
        var before=inventory(p);
        ItemStack remainder;
        if(input.get("kind").getAsString().equals("drop_on")) {
            p.setPos(pos.getX()+.5,pos.getY()+2,pos.getZ()+.5);
            p.setItemInHand(InteractionHand.MAIN_HAND,ItemStack.EMPTY);
            var entity=p.drop(offered,false);
            if(entity==null) throw new IllegalStateException("player drop failed");
            entity.setPos(pos.getX()+.5,pos.getY()+.8125,pos.getZ()+.5);
            entity.setDeltaMovement(Vec3.ZERO);
            level.getBlockState(pos).getBlock().updateEntityAfterFallOn(level,entity);
            remainder=entity.isRemoved()?ItemStack.EMPTY:entity.getItem().copy();
            // A single landing event. Rejected remainder is returned as an outcome rather
            // than retried by vanilla physics in later ticks or lost from the observation.
            entity.discard();
        } else {
            float yaw=input.get("yaw").getAsFloat();
            Direction face=Direction.byName(input.get("face").getAsString());
            p.setYRot(yaw);p.setYHeadRot(yaw);p.setXRot(0);
            p.setShiftKeyDown(input.get("sneaking").getAsBoolean());
            p.setPos(pos.getX()+.5,pos.getY()+1,pos.getZ()+2);
            p.gameMode.useItemOn(p,level,offered,InteractionHand.MAIN_HAND,
                new BlockHitResult(new Vec3(pos.getX()+.5,pos.getY()+.8125,pos.getZ()+.5),face,pos,false));
            remainder=saved.isEmpty()?ItemStack.EMPTY:p.getMainHandItem().copy();
        }
        JsonObject out=new JsonObject();BlockPos rel=pos.subtract(origin);
        out.add("pos",CreateFuzz.GSON.toJsonTree(new int[]{rel.getX(),rel.getY(),rel.getZ()}));
        out.add("offered",stack(saved));out.add("remainder",stack(remainder));
        JsonArray taken=new JsonArray();
        if(!input.get("kind").getAsString().equals("drop_on")) {
            var after=inventory(p);
            if(!saved.isEmpty()) after.merge(BuiltInRegistries.ITEM.getKey(saved.getItem()).toString(),saved.getCount(),Integer::sum);
            if(!remainder.isEmpty()) after.merge(BuiltInRegistries.ITEM.getKey(remainder.getItem()).toString(),-remainder.getCount(),Integer::sum);
            for(var e:after.entrySet()) {
                int count=e.getValue()-before.getOrDefault(e.getKey(),0);
                if(count>0) {JsonObject s=new JsonObject();s.addProperty("item",e.getKey());s.addProperty("count",count);taken.add(s);}
            }
        }
        out.add("taken",taken);outcomes.computeIfAbsent(p,k->new JsonArray()).add(out);
    }
}
