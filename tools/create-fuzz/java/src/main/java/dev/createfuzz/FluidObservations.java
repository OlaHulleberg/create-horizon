package dev.createfuzz;

import com.google.gson.*;
import com.simibubi.create.content.fluids.*;
import com.simibubi.create.content.fluids.pump.PumpBlockEntity;
import com.simibubi.create.foundation.blockEntity.behaviour.BlockEntityBehaviour;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import java.lang.reflect.Field;
import java.util.*;

/** Live connection observations. Reflection is restricted to the pinned MIT Create classes. */
final class FluidObservations {
    private static Field field(Class<?> type,String name) {
        try {var field=type.getDeclaredField(name);field.setAccessible(true);return field;}
        catch(ReflectiveOperationException error){throw new IllegalStateException(error);}
    }
    private static final Field SOURCE=field(PipeConnection.class,"source");
    private static final Field PREVIOUS=field(PipeConnection.class,"previousSource");
    private static final Field NETWORK=field(PipeConnection.class,"network");
    private static final Field NETWORK_SOURCE=field(FluidNetwork.class,"source");
    private static Class<?> openHandlerType(){try{return Class.forName(OpenEndedPipe.class.getName()+"$OpenEndFluidHandler",false,OpenEndedPipe.class.getClassLoader());}catch(ClassNotFoundException error){throw new IllegalStateException(error);}}
    private static final Class<?> OPEN_HANDLER=openHandlerType();
    private static void buffer(Object handler,Set<Object> seen,Map<String,Integer> fluids) {
        if(!OPEN_HANDLER.isInstance(handler)||!seen.add(handler))return;
        var held=((net.neoforged.neoforge.fluids.capability.IFluidHandler)handler).getFluidInTank(0);
        if(!held.isEmpty())fluids.merge(FluidIds.of(held),held.getAmount(),Integer::sum);
    }
    static JsonObject observe(ServerLevel level,BlockPos pos) {
        var behaviour=BlockEntityBehaviour.get(level,pos,FluidTransportBehaviour.TYPE);
        if(behaviour==null)return null;
        var out=new JsonObject();out.addProperty("pressure_model","stored_connections");var connections=new JsonObject();float maximum=0;
        for(var side:Direction.values()) {
            var connection=behaviour.interfaces==null?null:behaviour.interfaces.get(side);
            var c=new JsonObject();var pressure=new JsonArray();
            float inbound=connection==null?0:connection.getPressure().getFirst();
            float outward=connection==null?0:connection.getPressure().getSecond();
            pressure.add(inbound);pressure.add(outward);maximum=Math.max(maximum,Math.max(inbound,outward));c.add("pressure",pressure);
            var buffers=new TreeMap<String,Integer>();var seen=Collections.newSetFromMap(new IdentityHashMap<Object,Boolean>());
            if(connection!=null) {
                var flow=behaviour.getFlow(side);
                if(flow!=null) {
                    var f=new JsonObject();f.addProperty("fluid",FluidIds.of(flow.fluid));
                    f.addProperty("inbound",flow.inbound);f.addProperty("progress",flow.progress.getValue());f.addProperty("complete",flow.complete);c.add("flow",f);
                }
                try {
                    for(var field:new Field[]{SOURCE,PREVIOUS}) {
                        var source=(Optional<?>)field.get(connection);
                        if(source.orElse(null) instanceof OpenEndedPipe open)buffer(open.provideHandler().getCapability(),seen,buffers);
                    }
                    var network=(Optional<?>)NETWORK.get(connection);
                    if(network.isPresent()) {
                        var provider=(com.simibubi.create.foundation.ICapabilityProvider<?>)NETWORK_SOURCE.get(network.get());
                        if(provider!=null)buffer(provider.getCapability(),seen,buffers);
                    }
                }catch(IllegalAccessException error){throw new IllegalStateException(error);}
            }
            // All six sides are normalized. A nonexistent empty connection and an allocated
            // empty connection are the same observation; live stale flows remain visible.
            var bank=new JsonObject();bank.add("fluids",CreateFuzz.GSON.toJsonTree(buffers));c.add("buffer",bank);connections.add(side.getName(),c);
        }
        out.add("connections",connections);
        if(level.getBlockEntity(pos)instanceof PumpBlockEntity)out.addProperty("pump_pressure",maximum);
        return out;
    }
}
