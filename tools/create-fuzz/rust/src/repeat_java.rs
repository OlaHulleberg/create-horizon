//! Check Java repeatability both alone and at the seed's usual 128-case batch position.
use crate::{compare, generate, java};
use std::collections::BTreeSet;

pub fn run(rounds: usize, probes: &[crate::Scenario], compact: bool) {
    assert!(rounds >= 2 && !probes.is_empty());
    let dir = crate::fuzz_dir();
    let mut failures = 0;
    for probe in probes {
        failures += repeat(&dir, rounds, "alone", std::slice::from_ref(probe), &[0]);
    }
    if compact {
        for group in probes.chunks(4) {
            let selected: Vec<_> = (0..group.len()).collect();
            failures += repeat(&dir, rounds, "shifted-batch", group, &selected);
        }
        println!("Java repeatability: {failures} differing snapshots");
        assert_eq!(
            failures, 0,
            "Java snapshots must repeat without comparison exceptions"
        );
        return;
    }
    let seeds: Vec<_> = probes.iter().map(|probe| probe.seed).collect();
    let batches: BTreeSet<_> = seeds.iter().map(|seed| seed / 128 * 128).collect();
    for first in batches {
        let mut cases: Vec<_> = (first..first + 128).map(generate::generate).collect();
        for probe in probes {
            if (first..first + 128).contains(&probe.seed) {
                cases[(probe.seed - first) as usize] = probe.clone();
            }
        }
        let selected: Vec<_> = seeds
            .iter()
            .filter(|&&seed| (first..first + 128).contains(&seed))
            .map(|seed| (seed - first) as usize)
            .collect();
        failures += repeat(&dir, rounds, "batch", &cases, &selected);
    }
    println!("Java repeatability: {failures} differing snapshots");
    assert_eq!(
        failures, 0,
        "Java snapshots must repeat without comparison exceptions"
    );
}

fn repeat(
    dir: &std::path::Path,
    rounds: usize,
    mode: &str,
    cases: &[crate::Scenario],
    selected: &[usize],
) -> usize {
    let baseline = java::run(dir, cases);
    let mut failures = 0;
    for round in 1..rounds {
        let current = java::varied(
            dir,
            cases,
            [1, 7, 17][(round - 1) % 3],
            (baseline[0].save_phase + ((round - 1) % 3 + 1) as u8 * 3) % 10,
        );
        for &index in selected {
            let diffs = compare::diff(&baseline[index], &current[index]);
            println!(
                "Java repeat seed={} mode={mode} round={round} phases={}/{} differences={}",
                cases[index].seed,
                baseline[index].save_phase,
                current[index].save_phase,
                diffs.len()
            );
            if diffs.is_empty() {
                continue;
            }
            failures += 1;
            let found = std::path::PathBuf::from(std::env::var("FUZZ_FOUND").expect("use run.sh"));
            std::fs::create_dir_all(&found).unwrap();
            let record = serde_json::json!({"scenario":cases[index],"baseline":baseline[index],"current":current[index],"diffs":diffs});
            std::fs::write(
                found.join(format!("repeat-{}-{mode}-{round}.json", cases[index].seed)),
                serde_json::to_string_pretty(&record).unwrap(),
            )
            .unwrap();
        }
    }
    failures
}
