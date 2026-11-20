//! Public native fluid scheduler bridge. The host supplies the environmental physics on
//! both sides; no Java Create block-entity or kinetic ticks execute in the Rust host region.
use crate::scenario::Pos;
use serde::{Deserialize, Serialize};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[derive(Serialize, Deserialize)]
pub struct State {
    pub pos: Pos,
    pub state: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Facts {
    pub replaceable: bool,
    pub collision: bool,
    pub sturdy: u8,
    pub fluid: Option<crate::fluid_world::Observation>,
}
#[derive(Deserialize)]
pub struct Cell {
    pub pos: Pos,
    pub state: String,
    pub facts: Facts,
}
#[derive(Deserialize)]
pub struct Frame {
    pub states: Vec<Cell>,
    pub rules: crate::fluid_world::Rules,
    /// `FluidState.getFlow` at each requested cell, in request order.
    pub flows: Vec<[f64; 3]>,
}
#[derive(Serialize)]
pub struct Bucket {
    pub pos: Pos,
    pub item: String,
    pub face: String,
    pub hit: [f64; 3],
    pub creative: bool,
    pub sneaking: bool,
}
pub struct Host {
    owner: String,
    first: bool,
}
impl Host {
    pub fn new() -> Self {
        Self {
            owner: format!(
                "{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ),
            first: true,
        }
    }
    fn exchange(&self, request: serde_json::Value) -> serde_json::Value {
        let dir = crate::fuzz_dir();
        let name = format!(
            "fluid-host-{}-{}.json",
            self.owner,
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let tmp = dir.join("inbox").join(format!("{name}.tmp"));
        let path = dir.join("outbox").join(&name);
        std::fs::write(&tmp, serde_json::to_vec(&request).unwrap())
            .expect("fluid host inbox (requires the private Java server)");
        std::fs::rename(tmp, dir.join("inbox").join(&name)).unwrap();
        let start = Instant::now();
        loop {
            if let Ok(bytes) = std::fs::read(&path) {
                let _ = std::fs::remove_file(path);
                let result: serde_json::Value =
                    serde_json::from_slice(&bytes).expect("fluid host response");
                return result;
            }
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "fluid host timed out: {}",
                dir.display()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    pub fn tick(
        &mut self,
        clock: usize,
        changes: Vec<State>,
        manual: Vec<Pos>,
        native: Vec<Pos>,
        region: Vec<Pos>,
        flows: Vec<Pos>,
    ) -> Result<Frame, String> {
        self.frame(serde_json::json!({"clock":clock,"changes":changes,"manual":manual,"native":native,"region":region,"flows":flows}))
    }
    pub fn bucket(
        &mut self,
        clock: usize,
        changes: Vec<State>,
        region: Vec<Pos>,
        bucket: Bucket,
    ) -> Result<Frame, String> {
        self.frame(serde_json::json!({"clock":clock,"changes":changes,"manual":[],"native":[],"region":region,"flows":[],"bucket":bucket}))
    }
    fn frame(&mut self, mut request: serde_json::Value) -> Result<Frame, String> {
        request["owner"] = self.owner.clone().into();
        if self.first {
            request["begin"] = true.into();
            self.first = false;
        }
        let result = self.exchange(request);
        if let Some(error) = result.get("error") {
            let error = error.to_string();
            if error.contains("provide a contained fixture") {
                return Err(error);
            }
            panic!("fluid host: {error}");
        }
        Ok(serde_json::from_value(result).unwrap())
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if self.first {
            return;
        }
        let close = || {
            self.exchange(serde_json::json!({"owner":self.owner,"close":true}));
        };
        if std::thread::panicking() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(close));
        } else {
            close();
        }
    }
}
