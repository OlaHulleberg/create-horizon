package dev.createfuzz;
import com.google.gson.*;
import com.simibubi.create.content.fluids.drain.ItemDrainBlockEntity;
import com.simibubi.create.content.fluids.spout.SpoutBlockEntity;
import com.simibubi.create.content.fluids.pipes.SmartFluidPipeBlockEntity;
import com.simibubi.create.foundation.blockEntity.behaviour.BlockEntityBehaviour;
import com.simibubi.create.foundation.blockEntity.behaviour.fluid.SmartFluidTankBehaviour;
import com.simibubi.create.foundation.blockEntity.behaviour.filtering.FilteringBehaviour;
import com.simibubi.create.content.kinetics.belt.transport.TransportedItemStack;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.core.registries.BuiltInRegistries;

final class FluidMachineObservations {
 static void add(BlockEntity be,JsonObject snap) {
  if(be==null)return;
  if(be instanceof SmartFluidPipeBlockEntity) {
   var filter=BlockEntityBehaviour.get(be,FilteringBehaviour.TYPE);
   if(filter!=null&&!filter.getFilter().isEmpty())snap.addProperty("fluid_filter",BuiltInRegistries.ITEM.getKey(filter.getFilter().getItem()).toString());
  }
  if(!(be instanceof SpoutBlockEntity)&&!(be instanceof ItemDrainBlockEntity))return;
  var tank=BlockEntityBehaviour.get(be,SmartFluidTankBehaviour.TYPE);
  if(tank!=null) {
   var handler=tank.getPrimaryHandler();var storage=new JsonObject();storage.addProperty("capacity",handler.getCapacity());storage.addProperty("amount",handler.getFluidAmount());
   if(!handler.getFluid().isEmpty())storage.addProperty("fluid",FluidIds.of(handler.getFluid()));snap.add("fluid_storage",storage);
  }
  var machine=new JsonObject();
  if(be instanceof SpoutBlockEntity spout)machine.addProperty("processing",spout.processingTicks);
  else {
   machine.addProperty("processing",TransportObservations.<Integer>field(be,ItemDrainBlockEntity.class,"processingTicks"));
   TransportedItemStack held=TransportObservations.field(be,ItemDrainBlockEntity.class,"heldItem");
   if(held!=null&&!held.stack.isEmpty()) {
    var item=new JsonObject();item.addProperty("item",BuiltInRegistries.ITEM.getKey(held.stack.getItem()).toString());item.addProperty("count",held.stack.getCount());item.addProperty("position",held.beltPosition);item.addProperty("from",held.insertedFrom.getName());machine.add("held",item);
   }
  }
  snap.add("fluid_machine",machine);
 }
}
