//! Guided exploration orchestrates the independent collector, corpus and mutation primitives.
use crate::coverage::Collector;
use crate::{
    corpus::Corpus,
    generate::{self, Rng},
    java, mutate,
    scenario::Scenario,
};
use std::path::PathBuf;
pub fn run(rounds: u64) {
    let dir = crate::fuzz_dir();
    let report = std::env::var_os("FUZZ_COVERAGE_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("coverage"));
    std::fs::create_dir_all(report.join("cases")).unwrap();
    let mut collector = Collector::new(&report);
    let mut corpus = Corpus::open(&PathBuf::from(
        std::env::var("FUZZ_CORPUS").unwrap_or_else(|_| "corpus".into()),
    ));
    let replay = corpus.seeds.clone();
    // Cache only inputs actually repeated by the replay corpus. This also checks
    // Java-vs-Java when each individual execution happens to match the port.
    let mut frequency = std::collections::HashMap::<String, usize>::new();
    for case in &replay {
        *frequency
            .entry(serde_json::to_string(case).unwrap())
            .or_default() += 1;
    }
    let mut first = std::collections::HashMap::<String, crate::Snapshot>::new();
    let mut repeat_checks = 0;

    let mut rng = Rng::new(0);
    let mut covered = 0;
    let mut counts = crate::Counts::default();
    let mut history = String::from("round\tseed\tnew_branches\n");
    for round in 0..replay.len() as u64 + rounds {
        let replaying = round < replay.len() as u64;
        let seed = round - round.min(replay.len() as u64);
        let sc: Scenario = if replaying {
            replay[round as usize].clone()
        } else if !corpus.seeds.is_empty() && rng.chance(0.8) {
            mutate::mutate(rng.pick(&corpus.seeds), seed)
        } else {
            generate::generate(seed)
        };
        std::fs::write(
            report.join("cases").join(format!("{round}.json")),
            serde_json::to_string_pretty(&sc).unwrap(),
        )
        .unwrap();
        let (snapshot, exec) = java::covered(&dir, &sc);
        let key = serde_json::to_string(&sc).unwrap();
        if frequency.get(&key).copied().unwrap_or(0) > 1 {
            if let Some(previous) = first.get(&key) {
                let variation = crate::compare::diff(previous, &snapshot);
                assert!(
                    variation.is_empty(),
                    "Java changed across coverage replay for seed {}: {:?}",
                    sc.seed,
                    variation
                );
                repeat_checks += 1;
            } else {
                first.insert(key, snapshot.clone());
            }
        }

        let now = collector.measure(&exec);
        history.push_str(&format!("{round}\t{}\t{}\n", sc.seed, now - covered));
        let gained = now > covered;
        covered = now;
        // Reports may be on another filesystem from the server inbox/outbox.
        std::fs::copy(&exec, report.join("cases").join(format!("{round}.exec"))).unwrap();
        std::fs::remove_file(exec).unwrap();
        let supported = crate::record(&sc, &snapshot, &mut counts);
        if supported && gained && !replaying {
            corpus.keep(sc.clone());
        }
    }
    std::fs::write(report.join("seeds.tsv"), history).unwrap();
    counts.print();
    println!("{repeat_checks} Java-vs-Java replay comparisons matched");
    println!(
        "{}corpus: {} scenarios; report: {}",
        std::fs::read_to_string(report.join("summary.txt")).unwrap(),
        corpus.seeds.len(),
        report.display()
    );
}
