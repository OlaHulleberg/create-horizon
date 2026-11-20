package dev.createfuzz;
import com.google.gson.*;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;

/** The same explicit fluid containment domain on the Java and Rust-host worlds. */
final class FluidBounds {
 static boolean fluidItem(String item) { return item.endsWith("bucket") || item.equals("minecraft:honey_bottle") || item.equals("create:builders_tea"); }
 static final String OUTSIDE="fluid-world-outside-region";
 static boolean enabled(JsonObject scenario) {
  var all=new java.util.ArrayList<JsonObject>();
  for(var b:scenario.getAsJsonArray("blocks"))all.add(b.getAsJsonObject());
  if(scenario.has("events"))for(var b:scenario.getAsJsonArray("events"))all.add(b.getAsJsonObject());
  for(var b:all) {
   if(b.has("state")){String s=b.get("state").getAsString();String id=s.split("\\[",2)[0];if((!id.startsWith("create:")&&!java.util.Set.of("minecraft:air","minecraft:stone","minecraft:redstone_block").contains(id))||java.util.Set.of("create:honey","create:chocolate","create:limestone","create:scoria").contains(id)||s.contains("waterlogged=true"))return true;}
   if(b.has("action")) {
    var a=b.getAsJsonObject("action");
    if(a.has("item")&&fluidItem(a.get("item").getAsString()))return true;
    if(a.has("input")){var input=a.getAsJsonObject("input");if(input.has("stack")&&!input.get("stack").isJsonNull()&&fluidItem(input.getAsJsonObject("stack").get("item").getAsString()))return true;}
   }
  }return false;
 }
 static JsonArray region(JsonObject scenario) {
  int[] lo={24,24,24},hi={-24,-24,-24};
  var all=new java.util.ArrayList<JsonObject>();for(var b:scenario.getAsJsonArray("blocks"))all.add(b.getAsJsonObject());
  if(scenario.has("events"))for(var b:scenario.getAsJsonArray("events"))all.add(b.getAsJsonObject());
  var out=new JsonArray();if(all.isEmpty())return out;
  for(var b:all)for(int a=0;a<3;a++){int v=b.getAsJsonArray("pos").get(a).getAsInt();lo[a]=Math.min(lo[a],v);hi[a]=Math.max(hi[a],v);}
  for(int a=0;a<3;a++){lo[a]-=2;hi[a]+=2;}
  if(lo[0]<-24||lo[2]<-24||lo[1]<0||hi[0]>24||hi[2]>24||hi[1]>24)throw new IllegalArgumentException("fluid region exceeds slot");
  for(int x=lo[0];x<=hi[0];x++)for(int y=lo[1];y<=hi[1];y++)for(int z=lo[2];z<=hi[2];z++)out.add(CreateFuzz.GSON.toJsonTree(new int[]{x,y,z}));
  return out;
 }
 static void check(ServerLevel level,BlockPos origin,JsonArray region) {
  int[] lo={24,24,24},hi={-24,-24,-24};
  for(var value:region){var xyz=value.getAsJsonArray();for(int a=0;a<3;a++){lo[a]=Math.min(lo[a],xyz.get(a).getAsInt());hi[a]=Math.max(hi[a],xyz.get(a).getAsInt());}}
  for(var value:region){var xyz=value.getAsJsonArray();int x=xyz.get(0).getAsInt(),y=xyz.get(1).getAsInt(),z=xyz.get(2).getAsInt();
   if(x!=lo[0]&&x!=hi[0]&&y!=lo[1]&&y!=hi[1]&&z!=lo[2]&&z!=hi[2])continue;
   var pos=origin.offset(x,y,z);if(level.getFluidState(pos).isEmpty())continue;
   for(var d:Direction.values()){
    if(d==Direction.UP)continue;
    int nx=x+d.getStepX(),ny=y+d.getStepY(),nz=z+d.getStepZ();
    if(nx>=lo[0]&&nx<=hi[0]&&ny>=lo[1]&&ny<=hi[1]&&nz>=lo[2]&&nz<=hi[2])continue;
    if(level.getBlockState(pos.relative(d)).canBeReplaced())throw new IllegalArgumentException("fluid reaches the scenario boundary; provide a contained fixture");
   }
  }
 }
}
