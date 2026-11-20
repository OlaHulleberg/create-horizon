package dev.createfuzz;

import com.google.gson.JsonObject;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.InteractionResult;
import net.minecraft.world.item.BucketItem;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.GameType;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.Vec3;

/** Host-only bucket input: vanilla block use and the real item raycast, no Create handler use. */
final class WorldBucketInput {
    static void apply(ServerLevel level, ServerPlayer actor, BlockPos origin, JsonObject input) {
        var xyz=input.getAsJsonArray("pos");
        BlockPos pos=origin.offset(xyz.get(0).getAsInt(),xyz.get(1).getAsInt(),xyz.get(2).getAsInt());
        var hit=input.getAsJsonArray("hit");
        Vec3 point=new Vec3(pos.getX()+hit.get(0).getAsDouble(),pos.getY()+hit.get(1).getAsDouble(),pos.getZ()+hit.get(2).getAsDouble());
        Direction face=java.util.Objects.requireNonNull(Direction.byName(input.get("face").getAsString()));
        var item=BuiltInRegistries.ITEM.get(ResourceLocation.parse(input.get("item").getAsString()));
        if(!(item instanceof BucketItem))throw new IllegalArgumentException("not a world bucket");
        var stack=new ItemStack(item,1);
        actor.gameMode.changeGameModeForPlayer(input.get("creative").getAsBoolean()?GameType.CREATIVE:GameType.SURVIVAL);
        actor.setShiftKeyDown(input.get("sneaking").getAsBoolean());
        actor.setItemInHand(InteractionHand.MAIN_HAND,stack);
        actor.setItemInHand(InteractionHand.OFF_HAND,ItemStack.EMPTY);
        actor.setPos(pos.getX()+.5,pos.getY()+1,pos.getZ()+2);
        actor.setYRot(0);actor.setXRot(0);
        var result=InteractionResult.PASS;
        // Rust has already run the Create block-use boundary. Native cauldron and other
        // vanilla interactions still belong to the host, including secondary-use bypass.
        if(!BuiltInRegistries.BLOCK.getKey(level.getBlockState(pos).getBlock()).getNamespace().equals("create"))
            result=actor.gameMode.useItemOn(actor,level,stack,InteractionHand.MAIN_HAND,new BlockHitResult(point,face,pos,false));
        if(!result.consumesAction()) {
            Vec3 eye=point.add(Vec3.atLowerCornerOf(face.getNormal()).scale(2.5));
            actor.setPos(eye.x,eye.y-actor.getEyeHeight(),eye.z);
            actor.setYRot((float)Math.toDegrees(Math.atan2(face.getStepX(),-face.getStepZ())));
            actor.setXRot(face.getStepY()*90);
            actor.gameMode.useItem(actor,level,stack,InteractionHand.MAIN_HAND);
        }
    }
}
