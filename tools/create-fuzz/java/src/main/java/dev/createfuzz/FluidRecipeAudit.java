package dev.createfuzz;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.item.ItemStack;
import net.neoforged.neoforge.fluids.FluidStack;
import com.simibubi.create.content.processing.recipe.ProcessingRecipe;

/** Check the component-free recipe subset used by the engine adapter against loaded data. */
final class FluidRecipeAudit {
 static ProcessingRecipe<?,?> recipe(ServerLevel level,String id) {
  var holder=level.getRecipeManager().byKey(ResourceLocation.parse(id)).orElseThrow(()->new IllegalStateException("missing fluid recipe "+id));
  if(!(holder.value() instanceof ProcessingRecipe<?,?> recipe))throw new IllegalStateException("not a Create processing recipe "+id);
  return recipe;
 }
 static void check(ServerLevel level) {
  for(String[] row:new String[][]{{"builders_tea","create:builders_tea","create:tea"},{"honey_bottle","minecraft:honey_bottle","create:honey"}}) {
   var r=recipe(level,"create:emptying/"+row[0]);var input=new ItemStack(BuiltInRegistries.ITEM.get(ResourceLocation.parse(row[1])));
   if(!r.getIngredients().getFirst().test(input)||r.getFluidResults().size()!=1||r.getFluidResults().getFirst().getAmount()!=250||!FluidIds.of(r.getFluidResults().getFirst()).equals(row[2]))throw new IllegalStateException("emptying contract "+row[0]);
  }
  for(String[] row:new String[][]{{"builders_tea","minecraft:glass_bottle","create:tea","create:builders_tea"},{"honey_bottle","minecraft:glass_bottle","create:honey","minecraft:honey_bottle"},{"chocolate_glazed_berries","minecraft:sweet_berries","create:chocolate","create:chocolate_glazed_berries"}}) {
   var r=recipe(level,"create:filling/"+row[0]);var input=new ItemStack(BuiltInRegistries.ITEM.get(ResourceLocation.parse(row[1])));var fluid=new FluidStack(BuiltInRegistries.FLUID.get(ResourceLocation.parse(row[2])),250);
   if(!r.getIngredients().getFirst().test(input)||r.getFluidIngredients().getFirst().amount()!=250||!r.getFluidIngredients().getFirst().test(fluid)||!BuiltInRegistries.ITEM.getKey(r.getRollableResults().getFirst().getStack().getItem()).toString().equals(row[3]))throw new IllegalStateException("filling contract "+row[0]);
  }
 }
}
