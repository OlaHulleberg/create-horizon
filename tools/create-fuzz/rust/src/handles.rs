//! Valve entity data maintained by the glue outside CreateWorld.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Handle {
    pub selected: i32,
    pub remaining: u8,
    pub total: u8,
    pub target: i32,
    pub back: bool,
}
impl Default for Handle {
    fn default() -> Self {
        Self {
            selected: 45,
            remaining: 0,
            total: 0,
            target: 0,
            back: false,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub selected: i32,
    pub in_use: i32,
    pub angle: Option<f32>,
    #[serde(default)]
    pub angle_unavailable: Option<String>,
}
impl Handle {
    pub fn started(&mut self, back: bool) {
        self.total = create_engine::valve::use_ticks(self.selected);
        self.remaining = self.total;
        self.target = create_engine::valve::target_angle(self.selected);
        self.back = back;
    }
    pub fn observe(&self, positive: bool) -> Observation {
        Observation {
            selected: self.selected,
            in_use: i32::from(self.remaining),
            angle_unavailable: None,
            angle: Some(create_engine::valve::drawn_angle(
                self.target,
                self.total,
                f32::from(self.total - self.remaining),
                self.back,
                positive,
            )),
        }
    }
}
