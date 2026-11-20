//! The Java side: batches go through the test mod's inbox and come back from its outbox.

use crate::scenario::{Scenario, Snapshot};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicU64 = AtomicU64::new(0);
static BOOT: OnceLock<String> = OnceLock::new();

pub fn run(dir: &Path, cases: &[Scenario]) -> Vec<Snapshot> {
    exchange(dir, cases, false, None).0
}

/// Repeat in a different slot and a separate batch, retaining the full comparison.
/// Full batches rotate; smaller ones leave empty slots ahead of their real cases.
pub fn varied(dir: &Path, cases: &[Scenario], offset: usize, phase: u8) -> Vec<Snapshot> {
    assert!(!cases.is_empty() && cases.len() <= 128 && offset > 0);
    if cases.len() + offset <= 128 {
        let empty = Scenario {
            version: 3,
            seed: 0,
            day_time: cases[0].day_time,
            ticks: 1,
            checkpoints: vec![],
            blocks: vec![],
            events: vec![],
        };
        let mut shifted = vec![empty; offset];
        shifted.extend_from_slice(cases);
        return exchange(dir, &shifted, false, Some(phase))
            .0
            .into_iter()
            .skip(offset)
            .collect();
    }
    let rotation = offset % cases.len();
    let mut shifted = cases.to_vec();
    shifted.rotate_right(rotation);
    let mut snapshots = exchange(dir, &shifted, false, Some(phase)).0;
    snapshots.rotate_left(rotation);
    snapshots
}

pub fn covered(dir: &Path, case: &Scenario) -> (Snapshot, std::path::PathBuf) {
    let (mut snapshots, path) = exchange(dir, std::slice::from_ref(case), true, None);
    (snapshots.remove(0), path)
}

fn exchange(
    dir: &Path,
    cases: &[Scenario],
    coverage: bool,
    phase: Option<u8>,
) -> (Vec<Snapshot>, std::path::PathBuf) {
    for sc in cases {
        crate::catalogue::scenario(sc);
    }
    let runtime = dir.join("harness-runtime.json");
    let stamp: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&runtime)
        .unwrap_or_else(|_| panic!("missing Java runtime stamp at {}; the server owner must load the matching reviewed harness build", runtime.display())))
        .expect("Java runtime stamp");
    assert_eq!(
        stamp["protocol"].as_u64(),
        Some(1),
        "unsupported Java harness protocol"
    );
    let expected = std::env::var("FUZZ_HARNESS_HASH").expect("use run.sh");
    assert_eq!(
        stamp["source_hash"].as_str(),
        Some(expected.as_str()),
        "Java harness source mismatch: refuse comparisons against a different server build"
    );
    let boot = stamp["boot"].as_str().expect("Java boot id").to_owned();
    assert_eq!(
        BOOT.get_or_init(|| boot.clone()),
        &boot,
        "Java server restarted during this run; discard the mixed-runtime campaign"
    );
    let name = format!(
        "{}{}-{}-{}.json",
        if coverage { "cover-" } else { "" },
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let inbox = dir.join("inbox");
    let tmp = inbox.join(format!("{name}.tmp"));
    let mut wire = serde_json::to_value(cases).unwrap();
    if let Some(phase) = phase {
        assert!(phase < 10);
        for case in wire.as_array_mut().unwrap() {
            case["_start_phase"] = phase.into();
        }
    }
    std::fs::write(&tmp, serde_json::to_string(&wire).unwrap()).expect("write inbox");
    std::fs::rename(&tmp, inbox.join(&name)).expect("move into inbox");
    let out = dir.join("outbox").join(&name);
    let start = Instant::now();
    loop {
        if let Ok(file) = std::fs::File::open(&out) {
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(file);
            let first = reader
                .fill_buf()
                .expect("outbox read")
                .iter()
                .copied()
                .find(|b| !b.is_ascii_whitespace());
            if first == Some(b'[') {
                let after: serde_json::Value = serde_json::from_reader(
                    std::fs::File::open(&runtime).expect("runtime stamp after response"),
                )
                .expect("runtime stamp");
                assert_eq!(
                    after["boot"], stamp["boot"],
                    "Java restarted during request"
                );
                assert_eq!(
                    after["source_hash"], stamp["source_hash"],
                    "Java build changed during request"
                );
                let snapshots = serde_json::from_reader(reader).expect("snapshots");
                let _ = std::fs::remove_file(&out);
                return (snapshots, dir.join("outbox").join(format!("{name}.exec")));
            }
            #[derive(serde::Deserialize)]
            struct ErrorResponse {
                error: String,
            }
            let error: ErrorResponse = serde_json::from_reader(reader).expect("outbox error");
            let _ = std::fs::remove_file(&out);
            panic!("java side: {}", error.error);
        }
        if let Ok(pid) = std::fs::read_to_string(dir.join("server.pid"))
            && std::fs::read_to_string(format!("/proc/{}/stat", pid.trim()))
                .map_or(true, |st| st.split_whitespace().nth(2) == Some("Z"))
        {
            panic!(
                "the Java server exited: see {}",
                dir.join("server.log").display()
            );
        }
        if start.elapsed() > Duration::from_secs(600) {
            panic!("no answer from the Java side in 10 min: is `./run.sh server` running?");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Bound concurrently retained checkpoint cells on both the Java and Rust sides.
/// A single larger case runs alone; callers must consume each batch before requesting more.
pub fn batches(
    cases: &[Scenario],
    maximum: usize,
) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start == cases.len() {
            return None;
        }
        let begin = start;
        let mut cells = 0usize;
        while start < cases.len() && start - begin < maximum.max(1) {
            let next = cases[start]
                .region()
                .len()
                .saturating_mul(cases[start].checkpoints.len() + 1);
            if start > begin && cells.saturating_add(next) > 131_072 {
                break;
            }
            cells = cells.saturating_add(next);
            start += 1;
        }
        Some(begin..start)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn large_histories_run_alone_and_batching_keeps_order() {
        let mut heavy = crate::fluid_world::generate(17);
        heavy.ticks = 1000;
        heavy.checkpoints = (0..heavy.ticks).collect();
        println!(
            "BlockSnap inline bytes {}; heavy scenario cells {}",
            std::mem::size_of::<crate::observation::BlockSnap>(),
            heavy.region().len() * (heavy.checkpoints.len() + 1)
        );
        let cases = vec![
            heavy.clone(),
            heavy,
            crate::geometry::stress_boundary(0),
            crate::geometry::states(2, 1),
        ];
        let batches: Vec<_> = super::batches(&cases, 128).collect();
        assert_eq!(batches[0], 0..1);
        assert_eq!(batches[1], 1..2);
        assert_eq!(
            batches.into_iter().flatten().collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }
}
