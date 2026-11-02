//! Create's cart assembler (`CartAssemblerBlock`, `CartAssemblerBlockEntity`,
//! `MountedContraption.assemble`): a minecart passing over it picks up the structure above
//! (`onMinecartPass` → `assembleNextTick`) or sets it down, by the rail it stands in for and its
//! power; a powered one sends the cart on, an unpowered powered rail brakes it.
//!
//! Only the rules are here: Horizon has no hook where a minecart passes a mod's rail, and no
//! entity a contraption could ride on a cart (PORTING.md, "8 Contraptions").

use crate::kinetics::{Axis, Dir};

/// `CartAssembleRailType`: the rail the assembler stands in for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RailType {
    Regular,
    PoweredRail,
    DetectorRail,
    ActivatorRail,
    ControllerRail,
}

impl RailType {
    pub const ALL: [RailType; 5] = [
        RailType::Regular,
        RailType::PoweredRail,
        RailType::DetectorRail,
        RailType::ActivatorRail,
        RailType::ControllerRail,
    ];

    /// The `rail_type` property's value.
    pub fn name(self) -> &'static str {
        match self {
            RailType::Regular => "regular",
            RailType::PoweredRail => "powered_rail",
            RailType::DetectorRail => "detector_rail",
            RailType::ActivatorRail => "activator_rail",
            RailType::ControllerRail => "controller_rail",
        }
    }

    /// The rail's item, which a click with sets the type (`useItemOn`).
    pub fn item(self) -> &'static str {
        match self {
            RailType::Regular => "minecraft:rail",
            RailType::PoweredRail => "minecraft:powered_rail",
            RailType::DetectorRail => "minecraft:detector_rail",
            RailType::ActivatorRail => "minecraft:activator_rail",
            RailType::ControllerRail => "create:controller_rail",
        }
    }

    pub fn from_item(item: &str) -> Option<RailType> {
        RailType::ALL.into_iter().find(|t| t.item() == item)
    }
}

/// `CartAssemblerAction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Assemble,
    Disassemble,
    AssembleAccelerate,
    DisassembleBrake,
    AssembleAccelerateDirectional,
    Pass,
}

impl Action {
    pub fn assembles(self) -> bool {
        matches!(
            self,
            Action::Assemble | Action::AssembleAccelerate | Action::AssembleAccelerateDirectional
        )
    }
    pub fn disassembles(self) -> bool {
        matches!(self, Action::Disassemble | Action::DisassembleBrake)
    }
}

/// `getActionForCart`: what the assembler does with a cart, by its rail type and power;
/// `riders` is whether the cart carries anything (a detector rail's test).
pub fn action(rail: RailType, powered: bool, riders: bool) -> Action {
    match rail {
        RailType::ActivatorRail if powered => Action::Disassemble,
        RailType::ActivatorRail => Action::Pass,
        RailType::ControllerRail if powered => Action::AssembleAccelerateDirectional,
        RailType::ControllerRail => Action::DisassembleBrake,
        RailType::DetectorRail if riders => Action::Disassemble,
        RailType::DetectorRail => Action::AssembleAccelerate,
        RailType::PoweredRail if powered => Action::AssembleAccelerate,
        RailType::PoweredRail => Action::DisassembleBrake,
        RailType::Regular if powered => Action::Assemble,
        RailType::Regular => Action::Disassemble,
    }
}

/// `assemblyCooldown`: ticks between two carts it handles.
pub const COOLDOWN: u8 = 8;

/// NeoForge's `IBaseRailBlockExtension.getRailMaxSpeed` default.
pub const RAIL_MAX_SPEED: f64 = 0.4;

/// The assembler's block entity: ticks since it last handled a cart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assembler {
    pub since: u8,
}

impl Default for Assembler {
    fn default() -> Assembler {
        Assembler { since: COOLDOWN }
    }
}

impl Assembler {
    /// `tick` with the cart that passed it since the last (`cartToAssemble`): whether it acts
    /// on that cart now (`isMinecartUpdateValid`, then `resetTicksSinceMinecartUpdate`).
    pub fn tick(&mut self, cart: bool) -> bool {
        if self.since < COOLDOWN {
            self.since += 1;
        }
        if !cart || self.since < COOLDOWN {
            return false;
        }
        self.since = 0;
        true
    }
}

/// `CartAssemblerBlock.getHorizontalDirection`: the way a contraption it sets down faces.
pub fn horizontal_direction(rail_axis: Axis, backwards: bool) -> Dir {
    let pointing = if rail_axis == Axis::X {
        Dir::West
    } else {
        Dir::North
    };
    if backwards {
        pointing.opposite()
    } else {
        pointing
    }
}

/// The cart's new motion after `tryAssemble`, if the action sets one: `motion` is the cart's,
/// `facing` its `getMotionDirection`, `conductor(d)` whether the block toward `d` on the rail's
/// axis is a redstone conductor, `centre_offset` the assembler's centre minus the cart.
pub fn cart_motion(
    action: Action,
    rail_axis: Axis,
    backwards: bool,
    motion: [f64; 3],
    facing: Dir,
    conductor: impl Fn(Dir) -> bool,
    centre_offset: [f64; 3],
) -> Option<[f64; 3]> {
    match action {
        Action::AssembleAccelerate => {
            let length =
                (motion[0] * motion[0] + motion[1] * motion[1] + motion[2] * motion[2]).sqrt();
            if length <= 1.0 / 128.0 {
                return None;
            }
            let mut facing = facing;
            for d in [false, true].map(|p| Dir::from_axis(rail_axis, p)) {
                if conductor(d) {
                    facing = d.opposite();
                }
            }
            Some(facing.offset().map(|v| f64::from(v) * RAIL_MAX_SPEED))
        }
        // `ControllerRailBlock.getAccelerationVector` on a flat rail.
        Action::AssembleAccelerateDirectional => Some(
            horizontal_direction(rail_axis, backwards)
                .offset()
                .map(|v| f64::from(v) * RAIL_MAX_SPEED),
        ),
        Action::DisassembleBrake => Some([centre_offset[0] / 16.0, 0.0, centre_offset[2] / 16.0]),
        _ => None,
    }
}

/// A contraption's blocks: offset and state.
pub type Blocks = Vec<([i32; 3], u32)>;

/// `MountedContraption.assemble` from the assembler at `pos`: the structure above it (as any
/// contraption gathers it) and the assembler itself as a minecart anchor at offset 0. None when
/// nothing is above it (`blocks.size() == 1`).
pub fn gather(
    pos: [i32; 3],
    anchor_state: u32,
    at: impl Fn([i32; 3]) -> Option<crate::contraption::Found>,
) -> Result<Option<Blocks>, crate::contraption::Refused> {
    let above = [pos[0], pos[1] + 1, pos[2]];
    let (mut blocks, _) = crate::contraption::gather_from(above, pos, |p| p == pos, at)?;
    if blocks.is_empty() {
        return Ok(None);
    }
    blocks.push(([0, 0, 0], anchor_state));
    Ok(Some(blocks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contraption::Found;

    #[test]
    fn each_rail_type_acts_by_its_power() {
        use Action::*;
        let table = [
            (RailType::Regular, Assemble, Disassemble),
            (RailType::PoweredRail, AssembleAccelerate, DisassembleBrake),
            (RailType::ActivatorRail, Disassemble, Pass),
            (
                RailType::ControllerRail,
                AssembleAccelerateDirectional,
                DisassembleBrake,
            ),
        ];
        for (rail, on, off) in table {
            assert_eq!(action(rail, true, false), on, "{rail:?}");
            assert_eq!(action(rail, false, false), off, "{rail:?}");
        }
        assert_eq!(
            action(RailType::DetectorRail, true, false),
            AssembleAccelerate
        );
        assert_eq!(action(RailType::DetectorRail, false, true), Disassemble);
        assert!(AssembleAccelerate.assembles() && DisassembleBrake.disassembles());
        assert!(!Pass.assembles() && !Pass.disassembles());
        assert_eq!(
            RailType::from_item("minecraft:powered_rail"),
            Some(RailType::PoweredRail)
        );
    }

    #[test]
    fn it_handles_a_cart_at_most_every_eight_ticks() {
        let mut a = Assembler::default();
        assert!(a.tick(true));
        let waited = (1..=8).find(|_| a.tick(true));
        assert_eq!(waited, Some(8));
        assert!(!a.tick(false));
    }

    #[test]
    fn powered_assemblers_send_the_cart_on_and_brakes_draw_it_in() {
        // Moving east at 0.2 on an x rail; nothing beside it: on at 0.4 the way it went.
        let on = cart_motion(
            Action::AssembleAccelerate,
            Axis::X,
            false,
            [0.2, 0.0, 0.0],
            Dir::East,
            |_| false,
            [0.0; 3],
        );
        assert_eq!(on, Some([0.4, 0.0, 0.0]));
        // A conductor to the east sends it west.
        let back = cart_motion(
            Action::AssembleAccelerate,
            Axis::X,
            false,
            [0.2, 0.0, 0.0],
            Dir::East,
            |d| d == Dir::East,
            [0.0; 3],
        );
        assert_eq!(back, Some([-0.4, 0.0, 0.0]));
        let brake = cart_motion(
            Action::DisassembleBrake,
            Axis::X,
            false,
            [0.2, 0.0, 0.0],
            Dir::East,
            |_| false,
            [-0.32, 0.0, 0.0],
        );
        assert_eq!(brake, Some([-0.02, 0.0, 0.0]));
        assert_eq!(horizontal_direction(Axis::Z, true), Dir::South);
    }

    #[test]
    fn the_structure_above_rides_with_the_anchor() {
        let at = |p: [i32; 3]| {
            (p[0] == 0 && p[2] == 0 && (1..=2).contains(&p[1])).then(|| Found {
                state: 5,
                movable: true,
                sticky: Some(crate::contraption::Sticky::Slime),
                ..Default::default()
            })
        };
        let got = gather([0, 0, 0], 99, at).unwrap().unwrap();
        assert_eq!(got, vec![([0, 1, 0], 5), ([0, 2, 0], 5), ([0, 0, 0], 99)]);
        assert_eq!(gather([0, 0, 0], 99, |_| None), Ok(None));
    }
}
