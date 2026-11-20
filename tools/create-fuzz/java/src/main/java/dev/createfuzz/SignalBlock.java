package dev.createfuzz;

import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.StateDefinition;
import net.minecraft.world.level.block.state.properties.IntegerProperty;
import net.neoforged.neoforge.registries.DeferredBlock;
import net.neoforged.neoforge.registries.DeferredRegister;

/** Controlled external input. No wire propagation, support ticks or analog approximation. */
final class SignalBlock extends Block {
    static final IntegerProperty STRENGTH = IntegerProperty.create("strength", 0, 15);
    static final DeferredRegister.Blocks REGISTRY = DeferredRegister.createBlocks("createfuzz");
    static final DeferredBlock<SignalBlock> BLOCK =
            REGISTRY.register("signal", () -> new SignalBlock());

    SignalBlock() {
        super(Properties.of());
        registerDefaultState(defaultBlockState().setValue(STRENGTH, 0));
    }

    @Override
    protected void createBlockStateDefinition(StateDefinition.Builder<Block, BlockState> b) {
        b.add(STRENGTH);
    }

    @Override
    public boolean isSignalSource(BlockState s) {
        return true;
    }

    @Override
    public int getSignal(BlockState s, BlockGetter l, BlockPos p, Direction d) {
        return s.getValue(STRENGTH);
    }

    @Override
    public int getDirectSignal(BlockState s, BlockGetter l, BlockPos p, Direction d) {
        return s.getValue(STRENGTH);
    }
}
