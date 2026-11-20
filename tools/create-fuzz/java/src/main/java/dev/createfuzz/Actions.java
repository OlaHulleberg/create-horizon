package dev.createfuzz;

import com.google.gson.JsonObject;

import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.Vec3;

/** Player inputs enter the game-mode pipeline, including RightClickBlock and item placement. */
final class Actions {
    private static final java.util.Map<ServerPlayer, ItemStack> connectors = new java.util.WeakHashMap<>();
    static void speedSetting(ServerLevel level, ServerPlayer player, BlockPos pos, int row, int value) {
        if (row < 0 || row > 1 || value < 0 || value > 256)
            throw new IllegalArgumentException("invalid kinetic board selection");
        var be = level.getBlockEntity(pos);
        com.simibubi.create.foundation.blockEntity.behaviour.scrollValue.ScrollValueBehaviour behaviour;
        if (be instanceof com.simibubi.create.content.kinetics.motor.CreativeMotorBlockEntity motor)
            behaviour = motor.generatedSpeed;
        else if (be instanceof com.simibubi.create.content.kinetics.speedController.SpeedControllerBlockEntity controller)
            behaviour = controller.targetSpeed;
        else return;
        behaviour.setValueSettings(player,
                new com.simibubi.create.foundation.blockEntity.behaviour.ValueSettingsBehaviour.ValueSettings(row, value), false);
    }
    static void apply(ServerLevel level, ServerPlayer player, BlockPos pos, BlockPos origin, JsonObject action) {
        player.setShiftKeyDown(false);
        player.setItemInHand(InteractionHand.MAIN_HAND, ItemStack.EMPTY);
        player.setItemInHand(InteractionHand.OFF_HAND, ItemStack.EMPTY);
        if(action.get("kind").getAsString().equals("use") && action.get("item").getAsString().equals("minecraft:air")
            && (level.getBlockEntity(pos) instanceof com.simibubi.create.content.kinetics.belt.BeltBlockEntity
            || level.getBlockState(pos).getBlock() instanceof com.simibubi.create.content.logistics.depot.DepotBlock)) {
            JsonObject input=action.deepCopy();input.remove("item");input.add("stack",com.google.gson.JsonNull.INSTANCE);
            ItemInputs.apply(level,player,pos,origin,input);return;
        }
        switch (action.get("kind").getAsString()) {
            case "game_mode" -> player.gameMode.changeGameModeForPlayer(action.get("creative").getAsBoolean()?net.minecraft.world.level.GameType.CREATIVE:net.minecraft.world.level.GameType.SURVIVAL);
            case "item" -> ItemInputs.apply(level,player,pos,origin,action.getAsJsonObject("input"));
            case "capability" -> ItemInputs.capability(level,player,pos,origin,action);
            case "use" -> {
                Direction face = Direction.byName(action.get("face").getAsString());
                var local = action.getAsJsonArray("hit");
                Vec3 point =
                        new Vec3(
                                pos.getX() + local.get(0).getAsDouble(),
                                pos.getY() + local.get(1).getAsDouble(),
                                pos.getZ() + local.get(2).getAsDouble());
                var item =
                        BuiltInRegistries.ITEM.get(
                                ResourceLocation.parse(action.get("item").getAsString()));
                var stack = action.get("item").getAsString().equals("create:belt_connector")
                    ? connectors.computeIfAbsent(player, ignored -> new ItemStack(item, 64))
                    : new ItemStack(item, item instanceof net.minecraft.world.item.BucketItem ? 1 : 64);
                player.setYRot(action.get("yaw").getAsFloat());
                player.setYHeadRot(action.get("yaw").getAsFloat());
                player.setXRot(action.get("pitch").getAsFloat());
                player.setShiftKeyDown(action.get("sneaking").getAsBoolean());
                player.setPos(pos.getX() + .5, pos.getY() + 1, pos.getZ() + 2);
                player.setItemInHand(InteractionHand.MAIN_HAND, stack);
                var result = player.gameMode.useItemOn(
                        player,
                        level,
                        stack,
                        InteractionHand.MAIN_HAND,
                        new BlockHitResult(point, face, pos, false));
                if (item instanceof net.minecraft.world.item.BucketItem && !result.consumesAction()) {
                    // BucketItem uses the player's real ray cast through useItem, not useOn.
                    // Put the eye on the selected face normal and look through the supplied hit.
                    Vec3 eye = point.add(Vec3.atLowerCornerOf(face.getNormal()).scale(2.5));
                    player.setPos(eye.x, eye.y-player.getEyeHeight(), eye.z);
                    player.setYRot((float)Math.toDegrees(Math.atan2(face.getStepX(),-face.getStepZ())));
                    player.setXRot(face.getStepY()*90);
                    player.gameMode.useItem(player,level,stack,InteractionHand.MAIN_HAND);
                }
            }
            case "scroll_setting" -> {
                // ValueSettingsPacket.applySettings on the block's ScrollValueBehaviour (a brass
                // tunnel's selection mode, an ejector's stack size): setValueSettings.
                int row=action.get("row").getAsInt(), value=action.get("value").getAsInt();
                if (row<0 || row>1 || value<0 || value>64) throw new IllegalArgumentException("scroll board coordinates");
                var be=level.getBlockEntity(pos);
                var scroll=be==null?null:com.simibubi.create.foundation.blockEntity.behaviour.BlockEntityBehaviour.get(be,
                        com.simibubi.create.foundation.blockEntity.behaviour.scrollValue.ScrollValueBehaviour.TYPE);
                if (scroll!=null)
                    scroll.setValueSettings(player,new com.simibubi.create.foundation.blockEntity.behaviour.ValueSettingsBehaviour.ValueSettings(row,value),false);
            }
            case "filter_setting" -> {
                // ValueSettingsPacket.applySettings on the block's FilteringBehaviour, sent by the
                // client only while its amount board shows (acceptsValueSettings): a release of a
                // short click (onShortInteract with the main hand's stack) or a board pick
                // (setValueSettings).
                int row=action.get("row").getAsInt(), value=action.get("value").getAsInt();
                if (row<0 || row>1 || value<0 || value>64) throw new IllegalArgumentException("filter board coordinates");
                var be=level.getBlockEntity(pos);
                var filtering=be==null?null:com.simibubi.create.foundation.blockEntity.behaviour.BlockEntityBehaviour.get(be,
                        com.simibubi.create.foundation.blockEntity.behaviour.filtering.FilteringBehaviour.TYPE);
                if (filtering==null || !filtering.acceptsValueSettings()) return;
                var interact=action.get("interact");
                if (interact!=null && !interact.isJsonNull()) {
                    var i=interact.getAsJsonObject();
                    var item=BuiltInRegistries.ITEM.get(ResourceLocation.parse(i.get("item").getAsString()));
                    player.setItemInHand(InteractionHand.MAIN_HAND,new ItemStack(item,item instanceof net.minecraft.world.item.BucketItem?1:64));
                    Direction face=Direction.byName(i.get("face").getAsString());
                    filtering.onShortInteract(player,InteractionHand.MAIN_HAND,face,
                        new BlockHitResult(Vec3.atCenterOf(pos),face,pos,false));
                } else
                    filtering.setValueSettings(player,new com.simibubi.create.foundation.blockEntity.behaviour.ValueSettingsBehaviour.ValueSettings(row,value),false);
            }
            case "valve_setting" -> {
                int row=action.get("row").getAsInt(), value=action.get("value").getAsInt();
                if (row<0 || row>1 || value<0 || value>180) throw new IllegalArgumentException("valve board coordinates");
                if (level.getBlockEntity(pos) instanceof com.simibubi.create.content.kinetics.crank.ValveHandleBlockEntity valve)
                    valve.angleInput.setValueSettings(player,new com.simibubi.create.foundation.blockEntity.behaviour.ValueSettingsBehaviour.ValueSettings(row,value),false);
            }
            case "program" -> {
                var list = new java.util.Vector<com.simibubi.create.content.kinetics.transmission.sequencer.Instruction>();
                for (var element : action.getAsJsonArray("instructions")) {
                    var i = element.getAsJsonObject();
                    list.add(new com.simibubi.create.content.kinetics.transmission.sequencer.Instruction(
                        com.simibubi.create.content.kinetics.transmission.sequencer.SequencerInstructions.valueOf(i.get("instruction").getAsString()),
                        com.simibubi.create.content.kinetics.transmission.sequencer.InstructionSpeedModifiers.valueOf(i.get("modifier").getAsString()), i.get("value").getAsInt()));
                }
                player.setPos(pos.getX()+.5,pos.getY()+1,pos.getZ()+2);
                if (level.getBlockEntity(pos) instanceof com.simibubi.create.content.kinetics.transmission.sequencer.SequencedGearshiftBlockEntity)
                    new com.simibubi.create.content.kinetics.transmission.sequencer.ConfigureSequencedGearshiftPacket(pos,list).handle(player);
            }
            case "speed_setting" -> speedSetting(level, player, pos,
                    action.get("row").getAsInt(), action.get("value").getAsInt());
            case "chain_link" -> {
                var t=action.getAsJsonArray("target");
                BlockPos target=origin.offset(t.get(0).getAsInt(),t.get(1).getAsInt(),t.get(2).getAsInt());
                player.setPos(pos.getX()+.5,pos.getY()+1,pos.getZ()+2);
                player.setItemInHand(InteractionHand.MAIN_HAND,new ItemStack(net.minecraft.world.item.Items.CHAIN,64));
                new com.simibubi.create.content.kinetics.chainConveyor.ChainConveyorConnectionPacket(pos,target,new ItemStack(net.minecraft.world.item.Items.CHAIN),action.get("connect").getAsBoolean()).handle(player);
            }
            case "remove" -> player.gameMode.destroyBlock(pos);
            case "signal" -> {
                int strength = action.get("strength").getAsInt();
                if (strength < 0 || strength > 15)
                    throw new IllegalArgumentException("signal outside 0..15");
                level.setBlock(
                        pos,
                        SignalBlock.BLOCK
                                .get()
                                .defaultBlockState()
                                .setValue(SignalBlock.STRENGTH, strength),
                        3);
            }
            default -> throw new IllegalArgumentException("unsupported player action: " + action);
        }
    }
}
