package dev.createfuzz;
import net.minecraft.core.registries.BuiltInRegistries;
import net.neoforged.neoforge.fluids.FluidStack;
import com.simibubi.create.foundation.fluid.FluidHelper;
/** The port calls the semantic milk fluid minecraft:milk; 1.21.1 supplies it via the host. */
final class FluidIds {
 static String of(FluidStack stack) {
  if(FluidHelper.isTag(stack,net.neoforged.neoforge.common.Tags.Fluids.MILK))return "minecraft:milk";
  return BuiltInRegistries.FLUID.getKey(stack.getFluid()).toString();
 }
}
