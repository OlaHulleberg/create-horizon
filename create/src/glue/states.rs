//! Create's block states as the engine numbers them.
//!
//! The mod API names a state by its block and a process-local ordinal (`BlockState`), not by the
//! game's numeric id. The engine wants one contiguous run of `u32` ids so its hot tests stay one
//! compare (`create_engine::states`). This table gives each of Create's blocks a base in that run,
//! in `Block` order, and each state its block's base plus its ordinal; it maps both ways.
//!
//! Block keys are requested once at registration; the states and their properties are read
//! when the first world opens, from the host's block-state facts.

use create_engine::Block;
use create_engine::world::StateTable;
use mod_api::{BlockKey, BlockState, Host, HostBlockStates, Registrar};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{OnceLock, RwLock};

/// Where Create's ids start. Any value works; a large one keeps them apart from small numbers
/// the engine's tests use for vanilla blocks.
pub const FIRST: u32 = 1 << 20;

/// The key of each of Create's blocks, by `Block as usize`, requested in `register`.
static KEYS: OnceLock<Vec<BlockKey>> = OnceLock::new();

/// The content's partial-model blocks (`create:part_*`): never placed, one model per state, drawn
/// by block entity renderers for the parts that move on their own (a shaft half, a press head).
/// They are not engine blocks. Sorted, as `data/create/horizon/block/part_*.json`.
pub const PARTIALS: &[&str] = &[
    "part_analog_lever_handle",
    "part_andesite_door_fold_left",
    "part_andesite_door_fold_right",
    "part_arm_base",
    "part_arm_claw_base",
    "part_arm_claw_base_goggles",
    "part_arm_cog",
    "part_arm_lower_body",
    "part_arm_lower_claw_grip",
    "part_arm_upper_body",
    "part_arm_upper_claw_grip",
    "part_bearing_top",
    "part_bearing_top_wooden",
    "part_belt_diagonal_end",
    "part_belt_diagonal_middle",
    "part_belt_diagonal_start",
    "part_belt_end",
    "part_belt_end_bottom",
    "part_belt_middle",
    "part_belt_middle_bottom",
    "part_belt_pulley",
    "part_belt_start",
    "part_belt_start_bottom",
    "part_blaze_active",
    "part_blaze_goggles",
    "part_blaze_goggles_small",
    "part_blaze_idle",
    "part_blaze_inert",
    "part_blaze_rods_large",
    "part_blaze_rods_small",
    "part_blaze_super",
    "part_blaze_super_active",
    "part_blaze_super_rods_large",
    "part_blaze_super_rods_small",
    "part_bogey_drive",
    "part_bogey_drive_belt",
    "part_bogey_drive_wheel",
    "part_bogey_frame",
    "part_bogey_pin",
    "part_bogey_piston",
    "part_bogey_wheel",
    "part_boiler_gauge",
    "part_boiler_gauge_dial",
    "part_cogwheel_shaft",
    "part_cogwheel_shaftless",
    "part_contraption_controls_button",
    "part_contraption_controls_indicator_0",
    "part_contraption_controls_indicator_4",
    "part_copper_backtank_cogs",
    "part_copper_backtank_shaft",
    "part_copper_door_fold_left",
    "part_copper_door_fold_right",
    "part_coupling_attachment",
    "part_coupling_connector",
    "part_coupling_ring",
    "part_crafter_arrow",
    "part_crafter_belt",
    "part_crafter_belt_frame",
    "part_crafter_lid",
    "part_cuckoo_creeper",
    "part_cuckoo_hour_hand",
    "part_cuckoo_left_door",
    "part_cuckoo_minute_hand",
    "part_cuckoo_pig",
    "part_cuckoo_right_door",
    "part_deployer_hand_holding",
    "part_deployer_hand_pointing",
    "part_deployer_hand_punching",
    "part_deployer_pole",
    "part_desk_bell_bell",
    "part_desk_bell_plunger",
    "part_display_link_tube",
    "part_drill_head",
    "part_ejector_top",
    "part_elevator_belt",
    "part_elevator_belt_half",
    "part_elevator_coil",
    "part_elevator_magnet",
    "part_engine_connector",
    "part_engine_linkage",
    "part_engine_piston",
    "part_fan_propeller",
    "part_fluid_chocolate",
    "part_fluid_honey",
    "part_fluid_lava",
    "part_fluid_milk",
    "part_fluid_valve_pointer",
    "part_fluid_water",
    "part_gantry_cogs",
    "part_gauge_dial",
    "part_gauge_head_speed",
    "part_gauge_head_stress",
    "part_girder_segment_bottom",
    "part_girder_segment_middle",
    "part_girder_segment_top",
    "part_hand_crank_base",
    "part_hand_crank_handle",
    "part_harvester_blade",
    "part_haunted_bell",
    "part_hose_coil",
    "part_hose_half",
    "part_hose_half_magnet",
    "part_hose_magnet",
    "part_hose_rope",
    "part_large_cogwheel_shaftless",
    "part_large_water_wheel_acacia",
    "part_large_water_wheel_bamboo",
    "part_large_water_wheel_birch",
    "part_large_water_wheel_cherry",
    "part_large_water_wheel_crimson",
    "part_large_water_wheel_dark_oak",
    "part_large_water_wheel_extension_acacia",
    "part_large_water_wheel_extension_bamboo",
    "part_large_water_wheel_extension_birch",
    "part_large_water_wheel_extension_cherry",
    "part_large_water_wheel_extension_crimson",
    "part_large_water_wheel_extension_dark_oak",
    "part_large_water_wheel_extension_jungle",
    "part_large_water_wheel_extension_mangrove",
    "part_large_water_wheel_extension_oak",
    "part_large_water_wheel_extension_pale_oak",
    "part_large_water_wheel_extension_spruce",
    "part_large_water_wheel_extension_warped",
    "part_large_water_wheel_jungle",
    "part_large_water_wheel_mangrove",
    "part_large_water_wheel_oak",
    "part_large_water_wheel_pale_oak",
    "part_large_water_wheel_spruce",
    "part_large_water_wheel_warped",
    "part_logistics_hat",
    "part_millstone_cog",
    "part_mixer_head",
    "part_mixer_pole",
    "part_netherite_backtank_cogs",
    "part_netherite_backtank_shaft",
    "part_packager_hatch_closed",
    "part_packager_hatch_open",
    "part_packager_tray",
    "part_peculiar_bell",
    "part_pfi_middle",
    "part_pfi_middle_powered",
    "part_pfi_top",
    "part_pipe_casing",
    "part_pipe_connection_down",
    "part_pipe_connection_east",
    "part_pipe_connection_north",
    "part_pipe_connection_south",
    "part_pipe_connection_up",
    "part_pipe_connection_west",
    "part_pipe_drain_down",
    "part_pipe_drain_east",
    "part_pipe_drain_north",
    "part_pipe_drain_south",
    "part_pipe_drain_up",
    "part_pipe_drain_west",
    "part_pipe_rim_connector_down",
    "part_pipe_rim_connector_east",
    "part_pipe_rim_connector_north",
    "part_pipe_rim_connector_south",
    "part_pipe_rim_connector_up",
    "part_pipe_rim_connector_west",
    "part_pipe_rim_down",
    "part_pipe_rim_east",
    "part_pipe_rim_north",
    "part_pipe_rim_south",
    "part_pipe_rim_up",
    "part_pipe_rim_west",
    "part_postbox_flag",
    "part_press_head",
    "part_psi_middle",
    "part_psi_middle_powered",
    "part_psi_top",
    "part_pump_cog",
    "part_repackager_tray",
    "part_roller_frame",
    "part_roller_wheel",
    "part_rope_coil",
    "part_rope_half",
    "part_rope_half_magnet",
    "part_saw_blade_horizontal_active",
    "part_saw_blade_horizontal_inactive",
    "part_saw_blade_horizontal_reversed",
    "part_saw_blade_vertical_active",
    "part_saw_blade_vertical_inactive",
    "part_saw_blade_vertical_reversed",
    "part_schematicannon_connector",
    "part_schematicannon_pipe",
    "part_shaft",
    "part_shaft_half",
    "part_signal_indicator_off",
    "part_signal_indicator_on",
    "part_signal_panel",
    "part_signal_red_tube",
    "part_signal_white_tube",
    "part_signal_yellow_tube",
    "part_speed_controller_bracket",
    "part_spout_bottom",
    "part_spout_middle",
    "part_spout_top",
    "part_sticker_head",
    "part_symmetry_crossplane",
    "part_symmetry_plane",
    "part_symmetry_tripleplane",
    "part_table_cloth_andesite",
    "part_table_cloth_black",
    "part_table_cloth_blue",
    "part_table_cloth_brass",
    "part_table_cloth_brown",
    "part_table_cloth_copper",
    "part_table_cloth_cyan",
    "part_table_cloth_gray",
    "part_table_cloth_green",
    "part_table_cloth_light_blue",
    "part_table_cloth_light_gray",
    "part_table_cloth_lime",
    "part_table_cloth_magenta",
    "part_table_cloth_orange",
    "part_table_cloth_pink",
    "part_table_cloth_price_side",
    "part_table_cloth_price_top",
    "part_table_cloth_purple",
    "part_table_cloth_red",
    "part_table_cloth_white",
    "part_table_cloth_yellow",
    "part_toolbox_drawer",
    "part_toolbox_lid_black",
    "part_toolbox_lid_blue",
    "part_toolbox_lid_brown",
    "part_toolbox_lid_cyan",
    "part_toolbox_lid_gray",
    "part_toolbox_lid_green",
    "part_toolbox_lid_light_blue",
    "part_toolbox_lid_light_gray",
    "part_toolbox_lid_lime",
    "part_toolbox_lid_magenta",
    "part_toolbox_lid_orange",
    "part_toolbox_lid_pink",
    "part_toolbox_lid_purple",
    "part_toolbox_lid_red",
    "part_toolbox_lid_white",
    "part_toolbox_lid_yellow",
    "part_track_segment_left",
    "part_track_segment_right",
    "part_track_tie",
    "part_train_controls_cover",
    "part_train_controls_lever",
    "part_water_wheel",
    "part_water_wheel_acacia",
    "part_water_wheel_bamboo",
    "part_water_wheel_birch",
    "part_water_wheel_cherry",
    "part_water_wheel_crimson",
    "part_water_wheel_dark_oak",
    "part_water_wheel_jungle",
    "part_water_wheel_mangrove",
    "part_water_wheel_oak",
    "part_water_wheel_pale_oak",
    "part_water_wheel_spruce",
    "part_water_wheel_warped",
    "part_whistle_mouth_large",
    "part_whistle_mouth_medium",
    "part_whistle_mouth_small",
];

/// The key of each of `PARTIALS`, requested in `register`.
static PARTIAL_KEYS: OnceLock<Vec<BlockKey>> = OnceLock::new();

/// Requests the key of every block the engine knows (`create:<name>`).
pub fn register(r: &mut dyn Registrar) {
    let keys = Block::NAMES
        .iter()
        .map(|name| r.block_key(&format!("create:{name}")))
        .collect();
    // A second registration (a second game in the process) asks for the same keys.
    let _ = KEYS.set(keys);
    let partials = PARTIALS
        .iter()
        .map(|name| r.block_key(&format!("create:{name}")))
        .collect();
    let _ = PARTIAL_KEYS.set(partials);
}

/// The key of partial model `name` (`part_shaft_half`, or with its `create:` namespace).
pub fn partial(name: &str) -> Option<BlockKey> {
    let name = name.strip_prefix("create:").unwrap_or(name);
    let i = PARTIALS.binary_search(&name).ok()?;
    PARTIAL_KEYS.get()?.get(i).copied()
}

/// The key of `block`. Panics before `register`, which runs before any world.
pub fn key(block: Block) -> BlockKey {
    KEYS.get()
        .expect("Create's block keys are requested at registration")[block as usize]
}

/// A hasher for `BlockKey`, whose hash is one `u32` slot: the slot itself is the hash.
#[derive(Default)]
pub struct SlotHasher(u64);

impl Hasher for SlotHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(b);
        }
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = u64::from(n);
    }
}

type SlotMap<V> = HashMap<BlockKey, V, BuildHasherDefault<SlotHasher>>;

/// Both directions of the numbering, once a world has opened.
pub struct States {
    /// Each block's first id and its state count.
    base: SlotMap<(u32, u32)>,
    /// The API state of each id, `FIRST..`.
    by_id: Vec<BlockState>,
}

static STATES: RwLock<Option<States>> = RwLock::new(None);

impl States {
    /// Reads every state of Create's blocks from the host and builds the engine's table.
    /// `None` when Create's blocks are not in this game (the content is off).
    fn read(host: &dyn Host) -> Option<(States, StateTable)> {
        let keys = KEYS.get()?;
        let facts = host.block_states();
        let mut base = SlotMap::default();
        let mut by_id = Vec::new();
        let mut names = Vec::new();
        for (i, &key) in keys.iter().enumerate() {
            let Some(range) = facts.states_of(key) else {
                continue;
            };
            let first = FIRST + by_id.len() as u32;
            base.insert(key, (first, range.len()));
            for state in range.iter() {
                by_id.push(state);
                let text = host
                    .block_state_string(state)
                    .unwrap_or_else(|| format!("create:{}", Block::NAMES[i]));
                names.push(text);
            }
        }
        if by_id.is_empty() {
            return None;
        }
        let parsed: Vec<(&str, Vec<(&str, &str)>)> = names.iter().map(|t| split(t)).collect();
        let table = StateTable::new(
            FIRST,
            parsed
                .iter()
                .map(|(name, props)| (*name, props.iter().copied())),
        );
        Some((States { base, by_id }, table))
    }

    fn id(&self, state: BlockState) -> Option<u32> {
        let &(first, len) = self.base.get(&state.block())?;
        let ordinal = u32::from(state.ordinal());
        (ordinal < len).then_some(first + ordinal)
    }
}

/// `create:shaft[axis=y,waterlogged=false]` as its name and properties.
pub fn split(text: &str) -> (&str, Vec<(&str, &str)>) {
    match text.split_once('[') {
        None => (text, Vec::new()),
        Some((name, rest)) => {
            let props = rest
                .trim_end_matches(']')
                .split(',')
                .filter_map(|p| p.split_once('='))
                .collect();
            (name, props)
        }
    }
}

/// Builds the numbering for the world being opened (on the server, or on a client joining one)
/// and returns the engine's table, or `None` when Create's blocks are absent. The ids are the
/// same for every world and both sides of this process (the states never change while it runs),
/// so the numbering is kept once built.
pub fn open(host: &dyn Host) -> Option<StateTable> {
    let (states, table) = States::read(host)?;
    create_engine::states::set_range(FIRST, states.by_id.len() as u32);
    *STATES.write().unwrap_or_else(|e| e.into_inner()) = Some(states);
    Some(table)
}

/// The engine id of `state`, or `None` for a block that is not Create's. Hot: one map lookup.
#[inline]
pub fn id(state: BlockState) -> Option<u32> {
    STATES.read().ok()?.as_ref()?.id(state)
}

/// The engine id of `state`, or 0 (no Create block) for any other: the form the engine's
/// `block_changed` and friends take.
#[inline]
pub fn id_or_none(state: BlockState) -> u32 {
    id(state).unwrap_or(0)
}

/// The API state of engine id `id`.
pub fn state(id: u32) -> Option<BlockState> {
    let i = id.checked_sub(FIRST)? as usize;
    STATES.read().ok()?.as_ref()?.by_id.get(i).copied()
}

#[cfg(test)]
mod tests {
    use super::{PARTIALS, split};

    #[test]
    fn partials_are_the_content_s_part_blocks_sorted() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/data/create/horizon/block");
        let mut declared: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|n| Some(n.strip_suffix(".json")?.to_owned()))
            .filter(|n| n.starts_with("part_"))
            .collect();
        declared.sort();
        assert_eq!(
            PARTIALS,
            declared.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    #[test]
    fn splits_state_strings() {
        assert_eq!(split("create:shaft"), ("create:shaft", vec![]));
        assert_eq!(
            split("create:shaft[axis=y,waterlogged=false]"),
            (
                "create:shaft",
                vec![("axis", "y"), ("waterlogged", "false")]
            )
        );
    }
}
