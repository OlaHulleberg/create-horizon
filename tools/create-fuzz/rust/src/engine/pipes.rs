//! Fluid placement and interactions from glue/fluids/{place,pipes,plumbing,clicks}.
use super::*;
use create_engine::{
    Block,
    fluids::{self, Cell, Gate},
    kinetics::{Axis, Dir},
};
fn dir(d: Dir) -> String {
    format!("{d:?}").to_lowercase()
}
fn axis(a: Axis) -> String {
    format!("{a:?}").to_lowercase()
}
impl Run {
    /// FluidTransportBehaviour.cacheFlows/loadFlows for casing and wrench conversions.
    pub(super) fn replace_pipe_state(&mut self, p: Pos, state: u32) {
        let previous = self.w.dims[0].fluid_transport.pipes.remove(&p);
        self.set(p, state);
        if let Some(previous) = previous {
            self.w.dims[0].fluid_transport.pipes.insert(p, previous);
        }
    }

    pub(super) fn pipe_sides(&self, p: Pos) -> u8 {
        let (_, props) = parse_state(&self.name(self.at(p)));
        Dir::ALL
            .into_iter()
            .filter(|d| props.iter().any(|(k, v)| k == &dir(*d) && v == "true"))
            .fold(0, |s, d| s | d.bit())
    }
    pub(super) fn pipe_cell(&self, p: Pos) -> Cell {
        let Some((block, i)) = self.w.table.get(self.at(p)) else {
            let name = self.name(self.at(p));
            let (block, props) = parse_state(&name);
            return if fluids::vanilla_target(&block, props.iter().any(|(k, _)| k == "honey_level"))
            {
                Cell::VanillaTarget
            } else {
                Cell::Solid
            };
        };
        match block {
            Block::FluidPipe => Cell::Pipe(self.pipe_sides(p)),
            Block::EncasedFluidPipe => Cell::Encased(self.pipe_sides(p)),
            Block::SmartFluidPipe => i.facing.map_or(Cell::Solid, |f| {
                Cell::Axis(
                    fluids::smart_pipe_axis(i.face == Some(create_engine::steam::Face::Wall), f),
                    self.w.dims[0]
                        .plumbing
                        .filters
                        .get(&p)
                        .map_or(Gate::Open, |filter| filter.gate()),
                )
            }),
            Block::GlassFluidPipe => i.axis.map_or(Cell::Solid, |a| Cell::Axis(a, Gate::Open)),
            Block::MechanicalPump => i.facing.map_or(Cell::Solid, Cell::Pump),
            Block::FluidTank
            | Block::CreativeFluidTank
            | Block::Spout
            | Block::ItemDrain
            | Block::Basin => Cell::Tank,
            Block::FluidValve => i.facing.map_or(Cell::Solid, |f| {
                Cell::Axis(
                    fluids::valve_pipe_axis(f, i.along_first),
                    if i.enabled { Gate::Open } else { Gate::Closed },
                )
            }),
            _ => Cell::Solid,
        }
    }
    fn sides_state(&self, name: &str, sides: u8, water: bool) -> String {
        let props: Vec<_> = Dir::ALL
            .into_iter()
            .map(|d| (dir(d), (sides & d.bit() != 0).to_string()))
            .chain(
                (name != "create:encased_fluid_pipe")
                    .then(|| ("waterlogged".into(), water.to_string())),
            )
            .collect();
        crate::scenario::format_state(name, &props)
    }
    fn connect_pipe(&mut self, p: Pos, preferred: Option<Dir>) {
        if self.w.block_at(0, p) != Some(Block::FluidPipe) {
            return;
        }
        let old = self.pipe_sides(p);
        let sides = fluids::pipe_sides(
            old,
            |d| self.pipe_cell(add(p, d.offset())),
            preferred.unwrap_or(Dir::North),
        );
        let water = self
            .w
            .table
            .get(self.at(p))
            .is_some_and(|(_, i)| i.waterlogged);
        let next = self.sides_state("create:fluid_pipe", sides, water);
        self.set(p, self.id(&next));
    }
    pub(super) fn tick_pipe_shapes(&mut self) {
        let mut checks = std::mem::take(&mut self.w.dims[0].pipe_checks);
        checks.sort();
        checks.dedup();
        for p in checks {
            self.connect_pipe(p, None);
        }
    }
    pub(super) fn place_pipe(
        &mut self,
        p: Pos,
        item: &str,
        yaw: f32,
        pitch: f32,
        sneaking: bool,
    ) -> bool {
        let water = self.world_fluid(p) == Some((create_engine::tank::Fluid::Water, 0));
        let nearest = create_engine::place::ordered_by_nearest(yaw, pitch)[0];
        let next = match item {
            "create:fluid_pipe" => self.sides_state(
                item,
                fluids::pipe_sides(0, |d| self.pipe_cell(add(p, d.offset())), nearest),
                water,
            ),
            "create:mechanical_pump" => {
                let target = if sneaking {
                    nearest
                } else {
                    nearest.opposite()
                };
                let facing = match create_engine::place::preferred(false, |d| {
                    self.w
                        .has_shaft_towards(0, add(p, d.offset()), d.opposite())
                }) {
                    Some(p) if !sneaking => p.opposite(),
                    _ => target,
                };
                let mut best = None;
                for d in Dir::ALL {
                    if !fluids::connects(self.pipe_cell(add(p, d.offset())), d) {
                        continue;
                    }
                    let (a, b) = (d.offset(), target.offset());
                    let distance: i32 = (0..3).map(|i| (a[i] - b[i]).pow(2)).sum();
                    if best.is_some_and(|(_, old)| distance > old) {
                        continue;
                    }
                    best = Some((d, distance));
                }
                let facing = match best {
                    Some((d, _)) if d.axis() != target.axis() && !sneaking => d,
                    _ => facing,
                };
                format!("{item}[facing={},waterlogged={water}]", dir(facing))
            }
            "create:fluid_valve" => {
                let (facing, along) = fluids::valve_placement(
                    nearest,
                    sneaking,
                    create_engine::place::horizontal_direction(yaw),
                    |d| {
                        self.w
                            .has_shaft_towards(0, add(p, d.offset()), d.opposite())
                    },
                    |d| fluids::connects(self.pipe_cell(add(p, d.offset())), d),
                );
                format!(
                    "{item}[axis_along_first={along},enabled=false,facing={},waterlogged=false]",
                    dir(facing)
                )
            }
            "create:smart_fluid_pipe" => {
                use create_engine::steam::Face;
                let horizontal = create_engine::place::horizontal_direction(yaw);
                let (mut face, mut facing) = match nearest {
                    Dir::Up => (Face::Ceiling, horizontal),
                    Dir::Down => (Face::Floor, horizontal),
                    d => (Face::Wall, d.opposite()),
                };
                if let Some((f, d)) = fluids::smart_pipe_placement(
                    |d| fluids::connects(self.pipe_cell(add(p, d.offset())), d),
                    create_engine::place::ordered_by_nearest(yaw, pitch),
                    face,
                    facing,
                ) {
                    face = f;
                    facing = d;
                }
                format!(
                    "{item}[face={},facing={},waterlogged=false]",
                    face.name(),
                    dir(facing)
                )
            }
            "create:creative_fluid_tank" => format!("{item}[bottom=true,shape=window,top=true]"),
            "create:item_drain" | "create:spout" => item.to_owned(),
            "create:depot" => "create:depot[waterlogged=false]".into(),
            _ => return false,
        };
        self.set(p, self.id(&next));
        true
    }
    pub(super) fn pipe_use(
        &mut self,
        p: Pos,
        item: &str,
        face: Dir,
        hit: [f64; 3],
        sneaking: bool,
    ) -> bool {
        let Some((block, info)) = self.w.table.get(self.at(p)).map(|(b, i)| (b, *i)) else {
            return false;
        };
        if !matches!(
            block,
            Block::FluidPipe
                | Block::GlassFluidPipe
                | Block::EncasedFluidPipe
                | Block::MechanicalPump
        ) {
            return false;
        }
        if item == "create:copper_casing"
            && matches!(block, Block::FluidPipe | Block::GlassFluidPipe)
            && !sneaking
        {
            let sides = if block == Block::FluidPipe {
                self.pipe_sides(p)
            } else {
                let a = info.axis.unwrap();
                Dir::from_axis(a, true).bit() | Dir::from_axis(a, false).bit()
            };
            let next = self.sides_state("create:encased_fluid_pipe", sides, false);
            self.replace_pipe_state(p, self.id(&next));
            return true;
        }
        if item != "create:wrench" {
            return false;
        }
        if self.brackets.remove(&p).is_some() {
            self.mark_column(p);
            return true;
        }
        if sneaking {
            self.set(p, AIR);
            return true;
        }
        match block {
            Block::MechanicalPump => {
                let next = with_prop(
                    &self.name(self.at(p)),
                    "facing",
                    &dir(info.facing.unwrap().opposite()),
                );
                self.switch(p, self.id(&next));
            }
            Block::FluidPipe => {
                let a = fluids::straight_axis(self.pipe_sides(p)).or_else(|| {
                    let distance = |d: Dir| {
                        let o = d.offset();
                        (0..3)
                            .map(|i| (hit[i] - (0.5 + f64::from(o[i]) * 0.5)).powi(2))
                            .sum::<f64>()
                    };
                    Dir::ALL
                        .into_iter()
                        .filter(|d| d.axis() != face.axis())
                        .min_by(|a, b| distance(*a).total_cmp(&distance(*b)))
                        .map(|d| d.axis())
                });
                if let Some(a) = a.filter(|a| *a != face.axis()) {
                    let next = format!(
                        "create:glass_fluid_pipe[alt=false,axis={},waterlogged={}]",
                        axis(a),
                        info.waterlogged
                    );
                    self.replace_pipe_state(p, self.id(&next));
                }
            }
            Block::GlassFluidPipe | Block::EncasedFluidPipe => {
                let sides = if block == Block::GlassFluidPipe {
                    let a = info.axis.unwrap();
                    Dir::from_axis(a, true).bit() | Dir::from_axis(a, false).bit()
                } else {
                    self.pipe_sides(p)
                };
                let first = Dir::ALL
                    .into_iter()
                    .find(|d| sides & d.bit() != 0)
                    .unwrap_or(Dir::Up);
                let next = self.sides_state("create:fluid_pipe", sides, info.waterlogged);
                self.replace_pipe_state(p, self.id(&next));
                self.connect_pipe(p, Some(first));
            }
            _ => {}
        }
        true
    }
}

#[cfg(test)]
mod hit_context_tests {
    use super::*;
    use crate::actions::{Action, Face};
    #[test]
    fn secondary_copper_use_places_a_block_before_normal_encasing() {
        let sc: crate::scenario::Scenario = serde_json::from_value(serde_json::json!({
            "version":3,"ticks":40,"checkpoints":[21,31],
            "blocks":[{"pos":[0,3,0],"state":"minecraft:stone"}],
            "events":[
                {"tick":10,"pos":[0,3,0],"action":{"kind":"use","item":"create:fluid_pipe","face":"up","hit":[0.5,1,0.5],"yaw":0,"pitch":0,"sneaking":false}},
                {"tick":20,"pos":[0,4,0],"action":{"kind":"use","item":"create:copper_casing","face":"north","hit":[0.5,0.5,0],"yaw":0,"pitch":0,"sneaking":true}},
                {"tick":30,"pos":[0,4,0],"action":{"kind":"use","item":"create:copper_casing","face":"south","hit":[0.5,0.5,1],"yaw":0,"pitch":0,"sneaking":false}}
            ]
        })).unwrap();
        let trace = crate::engine::run(&sc);
        assert_eq!(
            trace.checkpoints[&21].blocks["0,4,-1"].state,
            "create:copper_casing"
        );
        assert!(
            trace.checkpoints[&21].blocks["0,4,0"]
                .state
                .starts_with("create:fluid_pipe[")
        );
        assert!(
            trace.checkpoints[&31].blocks["0,4,0"]
                .state
                .starts_with("create:encased_fluid_pipe[")
        );
    }
    #[test]
    fn glass_wrench_uses_the_native_world_hit_precision() {
        for (x, axis) in [(0, "y"), (64, "x"), (256, "y")] {
            let mut sc = crate::fluid_inputs::generate(19);
            sc.blocks.clear();
            sc.events.clear();
            let mut run = Run::new(&sc);
            run.native_origin = [x, 0, 0];
            let p = [1, 4, 0];
            run.set(p,run.id("create:fluid_pipe[down=true,east=true,north=false,south=false,up=false,waterlogged=false,west=false]"));
            run.action(
                p,
                &Action::Use {
                    item: "create:wrench".into(),
                    face: Face::South,
                    hit: [0.9, 0.1, 1.],
                    yaw: 0.,
                    pitch: 0.,
                    sneaking: false,
                },
            );
            assert!(
                run.name(run.at(p)).contains(&format!("axis={axis}")),
                "origin {x}: {}",
                run.name(run.at(p))
            );
        }
    }
}
