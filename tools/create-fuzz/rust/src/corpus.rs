//! Persistent scenarios; coverage is replayed each session against the current Create binary.
use crate::scenario::Scenario;
use std::path::{Path, PathBuf};
pub struct Corpus {
    dir: PathBuf,
    pub seeds: Vec<Scenario>,
}
impl Corpus {
    pub fn open(dir: &Path) -> Self {
        std::fs::create_dir_all(dir).unwrap();
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|p| p.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();
        let seeds = paths
            .iter()
            .map(|p| {
                serde_json::from_str(&std::fs::read_to_string(p).unwrap()).expect("corpus scenario")
            })
            .collect();
        Self {
            dir: dir.into(),
            seeds,
        }
    }
    pub fn keep(&mut self, sc: Scenario) {
        let path = self
            .dir
            .join(format!("{}-{}.json", sc.seed, self.seeds.len()));
        std::fs::write(path, serde_json::to_string_pretty(&sc).unwrap()).unwrap();
        self.seeds.push(sc);
    }
}
