//! A block's default state as the port declares it (`create/data/create/sunrise/block/<name>.json`,
//! made by `tools/create-data`): what a placement starts from where it sets nothing.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// The checkout the engine comes from (`run.sh` sets `CREATE_HORIZON_DIR`).
fn checkout() -> PathBuf {
    std::env::var_os("CREATE_HORIZON_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."))
}

/// A block's declared default: property, value.
type State = Vec<(String, String)>;

/// `default_state` of the port's block `id` (`create:<name>`), sorted by property.
pub fn declared(id: &str) -> State {
    static CACHE: OnceLock<Mutex<HashMap<String, State>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap().get(id) {
        return v.clone();
    }
    let name = id.strip_prefix("create:").unwrap_or(id);
    let path = checkout().join(format!("create/data/create/sunrise/block/{name}.json"));
    let path = if path.exists() {
        path
    } else {
        checkout().join(format!("create/data/create/horizon/block/{name}.json"))
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let doc: serde_json::Value = serde_json::from_str(&text).expect("block data");
    let mut out: State = doc["default_state"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
        .collect();
    out.sort();
    cache.lock().unwrap().insert(id.to_owned(), out.clone());
    out
}
