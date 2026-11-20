mod actions;
mod allow;
mod belt_surface;
mod belts;
mod brass;
mod brass_funnels;
mod catalogue;
mod chute_columns;
mod chutes;
mod compare;
mod corpus;
mod coverage;
mod defaults;
mod depot_outputs;
mod depots;
mod ejectors;
mod engine;
mod fluid_host;
mod fluid_inputs;
mod fluid_machines;
mod fluid_network;
mod fluid_pipes;
mod fluid_pumps;
mod fluid_recipes;
mod fluid_world;
mod fluids;
mod funnel_push;
mod funnels;
mod generate;
mod geometry;
mod guided;
mod handles;
mod java;
mod logistics;
mod mutate;
mod network_inputs;
mod observation;
mod persistence;
mod press_processing;
mod program;
mod records;
mod repeat_java;
mod scenario;
mod shrink;
mod slicer;
mod steam_engine;
mod transmission;
mod transport;
mod trig;
mod tunnels;
mod waterwheel;

use scenario::{Scenario, Snapshot};
use std::path::{Path, PathBuf};

const HELP: &str = "\
create-fuzz <command>
  compare-records <main.json> <branch.json> audit recorded runs against one Java trace
  diff-files <java.json> <rust.json> compare recorded snapshots offline
  rust <case.json>               run one case through the port's engine, print its snapshot
  java <case.json>               run one case on the Java server, print its snapshot
  check <case.json>...           run cases on both sides and compare them
  gen <seed>                     print the generated case for a seed
  fuzz <first-seed> <count> [batch]  generate, run and compare; mismatches go to $FUZZ_FOUND (found/)
  manifest <case.json>...        report 73-id domains and generated/manual/interaction coverage
  cover <rounds>                coverage-guided cases, corpus/ and work/coverage reports
  fuzz-lane <first> <count> <lane> <stride> [batch]  compare one generator lane
  repeat-java <rounds> [--compact] <seed|case>...  compare repeated Java runs alone and in normal batches
  shrink <case.json> [witness]   shrink preserving a difference substring; writes <case>.min.json
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprint!("{HELP}");
        std::process::exit(2);
    };
    match cmd.as_str() {
        "compare-records" => {
            #[derive(serde::Deserialize)]
            struct Record {
                java: Snapshot,
                rust: Snapshot,
            }
            let read = |path: &str| -> Record {
                serde_json::from_reader(records::reader(Path::new(path))).expect("record JSON")
            };
            let main = read(&args[1]);
            let branch = read(&args[2]);
            let result = serde_json::json!({
                "java_changes": compare::diff(&main.java, &branch.java),
                "main": compare::diff(&main.java, &main.rust),
                "branch_same_java": compare::diff(&main.java, &branch.rust),
                "branch_own_java": compare::diff(&branch.java, &branch.rust),
            });
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
        }
        "diff-files" => {
            let read = |path: &str| -> Snapshot {
                serde_json::from_reader(records::reader(Path::new(path))).expect("snapshot JSON")
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&compare::diff(&read(&args[1]), &read(&args[2])))
                    .unwrap()
            );
        }
        "manifest" => {
            let manual: Vec<_> = args[1..].iter().map(|p| load(p)).collect();
            let generated: Vec<_> = (0..2000).map(generate::generate).collect();
            catalogue::report(&manual, &generated);
        }
        "rust" => print(&engine::run_at_phase(
            &load(&args[1]),
            args.get(2).map_or(0, |s| s.parse().expect("phase 0..9")),
        )),
        "java" => print(&java::run(&fuzz_dir(), &[load(&args[1])]).remove(0)),
        "gen" => println!(
            "{}",
            serde_json::to_string_pretty(&generate::generate(args[1].parse().unwrap())).unwrap()
        ),
        "fuzz-lane" => {
            let first: u64 = args[1].parse().unwrap();
            let count: u64 = args[2].parse().unwrap();
            let lane: u64 = args[3].parse().unwrap();
            let stride: u64 = args[4].parse().unwrap();
            let batch = args.get(5).map_or(4, |value| value.parse().unwrap());
            assert!(stride > 0 && lane < stride);
            let seeds: Vec<_> = (first..first + count)
                .filter(|seed| seed % stride == lane)
                .collect();
            fuzz_seeds(&seeds, batch);
        }
        "repeat-java" => {
            let rounds = args[1].parse().unwrap();
            let compact = args.iter().any(|arg| arg == "--compact");
            let probes: Vec<_> = args[2..]
                .iter()
                .filter(|arg| arg.as_str() != "--compact")
                .map(|seed| {
                    if seed.ends_with(".json") || seed.ends_with(".json.gz") {
                        load(seed)
                    } else {
                        generate::generate(seed.parse().unwrap())
                    }
                })
                .collect();
            repeat_java::run(rounds, &probes, compact);
        }
        "cover" => guided::run(args[1].parse().unwrap()),
        "fuzz" => {
            let first: u64 = args[1].parse().unwrap();
            let count: u64 = args[2].parse().unwrap();
            let batch: usize = args.get(3).map_or(128, |b| b.parse().unwrap());
            fuzz(first, count, batch);
        }
        "shrink" => {
            let (sc, d) =
                shrink::shrink(&fuzz_dir(), load(&args[1]), args.get(2).map(String::as_str));
            let out = args[1]
                .trim_end_matches(".gz")
                .trim_end_matches(".json")
                .to_string()
                + ".min.json";
            let record = serde_json::json!({"scenario": sc, "diffs": d});
            std::fs::write(&out, serde_json::to_string_pretty(&record).unwrap()).unwrap();
            println!("{}", serde_json::to_string_pretty(&record).unwrap());
        }
        "check" => {
            let cases: Vec<Scenario> = args[1..].iter().map(|p| load(p)).collect();
            let failed = check(&cases, &args[1..]);
            std::process::exit(i32::from(failed > 0));
        }
        _ => {
            eprint!("{HELP}");
            std::process::exit(2);
        }
    }
}

fn check(cases: &[Scenario], names: &[String]) -> usize {
    let mut failed = 0;
    for range in java::batches(cases, 128) {
        let java = java::run(&fuzz_dir(), &cases[range.clone()]);
        for ((sc, j), name) in cases[range.clone()].iter().zip(&java).zip(&names[range]) {
            let r = engine::run_like(sc, j);
            let diffs = compare::diff(j, &r);
            if diffs.is_empty() {
                println!("{name}: match ({} blocks)", j.blocks.len());
            } else {
                failed += 1;
                println!("{name}: {} differences", diffs.len());
                for d in &diffs {
                    println!("  {d}");
                }
                for (k, b) in &j.blocks {
                    if diffs.iter().any(|d| d.starts_with(&format!("{k}:"))) {
                        println!("  java {k} debug: {}", b.debug.as_deref().unwrap_or("-"));
                    }
                }
            }
        }
    }
    failed
}

/// Seeds `first..first + count`, in batches the Java side runs side by side. Each mismatch is
/// saved as `found/<seed>.json`: the case, both snapshots and the differences.
fn fuzz(first: u64, count: u64, batch: usize) {
    let seeds: Vec<u64> = (first..first + count).collect();
    fuzz_seeds(&seeds, batch);
}

fn fuzz_seeds(seeds: &[u64], batch: usize) {
    let mut counts = Counts::default();
    for chunk in seeds.chunks(batch) {
        let cases: Vec<Scenario> = chunk.iter().map(|&s| generate::generate(s)).collect();
        for range in java::batches(&cases, batch) {
            let java = java::run(&fuzz_dir(), &cases[range.clone()]);
            let group = &cases[range];
            let mut unstable = vec![false; group.len()];
            for (offset, delta) in [(1, 3), (17, 7)] {
                let replay = java::varied(
                    &fuzz_dir(),
                    group,
                    offset,
                    (java[0].save_phase + delta) % 10,
                );
                for (i, ((sc, j), again)) in group.iter().zip(&java).zip(&replay).enumerate() {
                    if !unstable[i] && !compare::diff(j, again).is_empty() {
                        unstable[i] = true;
                        counts.unstable += 1;
                        #[derive(serde::Serialize)]
                        struct Unstable<'a> {
                            scenario: &'a Scenario,
                            java: &'a Snapshot,
                            java_again: [&'a Snapshot; 1],
                        }
                        let found = PathBuf::from(std::env::var("FUZZ_FOUND").expect("use run.sh"));
                        std::fs::create_dir_all(&found).unwrap();
                        write_record(
                            &found.join(format!("unstable-{}.json", sc.seed)),
                            &Unstable {
                                scenario: sc,
                                java: j,
                                java_again: [again],
                            },
                        );
                        println!("seed {}: java unstable across batch contexts", sc.seed);
                    }
                }
            }
            for (i, (sc, j)) in group.iter().zip(&java).enumerate() {
                if !unstable[i] {
                    record_inner(sc, j, &mut counts, false);
                }
            }
        }
    }
    counts.print();
}

pub fn fuzz_dir() -> PathBuf {
    std::env::var_os("FUZZ_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../work"))
}

fn load(path: &str) -> Scenario {
    #[derive(serde::Deserialize)]
    struct Wrapped {
        scenario: Scenario,
    }
    // Found records can contain hundreds of MB of observations. Deserialize only
    // their scenario; Serde skips the other fields without building a Value tree.
    let open = || records::reader(Path::new(path));
    if let Ok(record) = serde_json::from_reader::<_, Wrapped>(open()) {
        return record.scenario;
    }
    serde_json::from_reader(open()).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn write_record(path: &Path, value: &impl serde::Serialize) {
    records::write(
        path,
        value,
        std::env::var_os("FUZZ_RECORD_GZIP").is_none_or(|v| v != "0"),
    );
}

fn print(s: &Snapshot) {
    let stdout = std::io::stdout();
    let mut writer = std::io::BufWriter::new(stdout.lock());
    serde_json::to_writer_pretty(&mut writer, s).unwrap();
    std::io::Write::write_all(&mut writer, b"\n").unwrap();
}

#[derive(Default)]
struct Counts {
    matched: usize,
    differed: usize,
    unstable: usize,
    unsupported: usize,
}
impl Counts {
    fn print(&self) {
        println!(
            "{} matched, {} differed, {} java-unstable, {} driver-unsupported",
            self.matched, self.differed, self.unstable, self.unsupported
        );
    }
}
fn record(sc: &Scenario, j: &Snapshot, counts: &mut Counts) -> bool {
    record_inner(sc, j, counts, true)
}
fn record_inner(sc: &Scenario, j: &Snapshot, counts: &mut Counts, verify_java: bool) -> bool {
    let found = std::env::var_os("FUZZ_FOUND")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../found"));
    std::fs::create_dir_all(&found).unwrap();
    let r = engine::run_like(sc, j);
    let diffs = compare::diff(j, &r);
    if [j, &r].iter().any(|snapshot| {
        snapshot
            .unavailable
            .iter()
            .any(|s| s == crate::fluid_world::OUTSIDE)
    }) {
        counts.unsupported += 1;
        #[derive(serde::Serialize)]
        struct Unsupported<'a> {
            kind: &'static str,
            scenario: &'a Scenario,
            java: &'a Snapshot,
            rust: &'a Snapshot,
            diffs: &'a [String],
        }
        write_record(
            &found.join(format!("unsupported-{}.json", sc.seed)),
            &Unsupported {
                kind: "driver-unsupported",
                scenario: sc,
                java: j,
                rust: &r,
                diffs: &diffs,
            },
        );
        println!("seed {}: driver unsupported (fluid containment)", sc.seed);
        return false;
    }
    // Recheck sequentially: three full checkpoint histories must not stay live
    // together with the original Java/Rust pair. One differing rerun proves instability.
    for (index, (offset, delta)) in [(1, 3), (7, 5), (17, 7)]
        .into_iter()
        .take(if verify_java { 3 } else { 0 })
        .enumerate()
    {
        let rerun = java::varied(
            &fuzz_dir(),
            std::slice::from_ref(sc),
            offset,
            (j.save_phase + delta) % 10,
        )
        .pop()
        .unwrap();
        let disagreement = compare::diff(j, &rerun);
        if !disagreement.is_empty() {
            counts.unstable += 1;
            println!(
                "Java variation seed {} rerun {}: {:?}",
                sc.seed, index, disagreement
            );
            #[derive(serde::Serialize)]
            struct Unstable<'a> {
                scenario: &'a Scenario,
                java: &'a Snapshot,
                java_again: [&'a Snapshot; 1],
            }
            write_record(
                &found.join(format!("unstable-{}.json", sc.seed)),
                &Unstable {
                    scenario: sc,
                    java: j,
                    java_again: [&rerun],
                },
            );
            println!("seed {}: java unstable", sc.seed);
            return true;
        }
    }
    if diffs.is_empty() {
        counts.matched += 1;
        return true;
    }
    counts.differed += 1;
    #[derive(serde::Serialize)]
    struct Difference<'a> {
        scenario: &'a Scenario,
        java: &'a Snapshot,
        rust: &'a Snapshot,
        diffs: &'a [String],
    }
    write_record(
        &found.join(format!("{}.json", sc.seed)),
        &Difference {
            scenario: sc,
            java: j,
            rust: &r,
            diffs: &diffs,
        },
    );
    println!(
        "seed {} ({}): {} differences, first: {}",
        sc.seed,
        generate::family_name(sc.seed),
        diffs.len(),
        diffs[0]
    );
    true
}
