//! `create-data compile`: reads Create's MIT data (`src/generated/`) and registration Java once,
//! and writes the port's committed outputs: the content mod's block and item declarations
//! (`content/data/create/horizon/`) and the engine's typed block table
//! (`mixins/create/engine/src/generated.rs`).
//!
//! `create-data prepare`: builds the playable content mod from a Create jar (or folders standing in
//! for one), converting 1.21.1 formats to 26.3. See `prepare.rs`.

mod blocks;
mod common_tags;
mod java;
mod prepare;
mod shapes;

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn usage() -> ! {
    eprintln!(
        "usage:\n  create-data compile --create <Create checkout> --out <mod repo root>\n  \
         create-data prepare --out <content mod folder> <jar or folder>..."
    );
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| -> Option<PathBuf> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };
    match args.first().map(String::as_str) {
        Some("compile") => {
            let create = flag("--create").unwrap_or_else(|| usage());
            let out = flag("--out").unwrap_or_else(|| usage());
            compile(&create, &out);
        }
        Some("prepare") => {
            let out = flag("--out").unwrap_or_else(|| usage());
            let mut sources = Vec::new();
            let mut skip = false;
            for a in &args[1..] {
                if skip {
                    skip = false;
                } else if a == "--out" {
                    skip = true;
                } else {
                    sources.push(PathBuf::from(a));
                }
            }
            if sources.is_empty() {
                usage();
            }
            let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
            match prepare::prepare(&content, &sources, &out) {
                Ok(report) => println!("{report}"),
                Err(e) => {
                    eprintln!("create-data prepare: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => usage(),
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    if fs::read(path).ok().as_deref() != Some(bytes) {
        fs::write(path, bytes).unwrap();
    }
}

fn pretty(v: &Value) -> Vec<u8> {
    let mut s = serde_json::to_string_pretty(v).unwrap();
    s.push('\n');
    s.into_bytes()
}

fn compile(create: &Path, out: &Path) {
    let generated = create.join("src/generated/resources");
    let java_root = create.join("src/main/java");
    let block_java = java::read_all(&java_root);
    let item_java = java::read_items(&java_root);
    let all = blocks::read_blocks(&generated, &block_java);
    let shapes = BTreeMap::new();

    let decl_dir = out.join("content/data/create/horizon");
    let _ = fs::remove_dir_all(&decl_dir);
    let mut pending: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut declared = Vec::new();
    for b in &all {
        let d = blocks::decide(b, &shapes);
        match blocks::declaration(b, &d) {
            Some(doc) => {
                write(
                    &decl_dir.join(format!("block/{}.json", b.name)),
                    &pretty(&doc),
                );
                declared.push(b.name.clone());
            }
            None => {
                if let blocks::Decision::Pending(why) = d {
                    pending.entry(why).or_default().push(b.name.clone());
                }
            }
        }
    }

    // Partial models as hidden blocks, with their own blockstates.
    let states_dir = out.join("content/assets/create/blockstates");
    let _ = fs::remove_dir_all(&states_dir);
    for (name, model, points) in blocks::PARTIALS {
        let (decl, states) = blocks::partial_files(model, points);
        write(&decl_dir.join(format!("block/{name}.json")), &pretty(&decl));
        write(&states_dir.join(format!("{name}.json")), &pretty(&states));
    }
    let models_dir = out.join("content/assets/create/models/block");
    for (name, model) in blocks::fluid_models() {
        write(&models_dir.join(format!("{name}.json")), &pretty(&model));
    }

    // Items that are not blocks: plain ingredients now, the rest with their subsystem.
    let block_names: std::collections::BTreeSet<&str> =
        all.iter().map(|b| b.name.as_str()).collect();
    let mut items = Vec::new();
    for (name, facts) in &item_java {
        if block_names.contains(name.as_str()) {
            continue;
        }
        if facts.class.as_deref() == Some("Item") && !facts.food {
            let doc = json!({ "template": "minecraft:stick" });
            write(&decl_dir.join(format!("item/{name}.json")), &pretty(&doc));
            items.push(name.clone());
        } else if name == "wrench" {
            // WrenchItem: `stacksTo(1)`; its use is the native mod's.
            let doc = json!({ "template": "minecraft:stick", "max_stack_size": 1 });
            write(&decl_dir.join(format!("item/{name}.json")), &pretty(&doc));
            items.push(name.clone());
        } else {
            let why = format!(
                "item {}: ported with its subsystem",
                facts.class.as_deref().unwrap_or("?")
            );
            pending.entry(why).or_default().push(name.clone());
        }
    }

    write(
        &out.join("content/horizon-mod.toml"),
        b"id = \"create\"\nname = \"Create\"\nversion = \"6.0.11\"\n\
description = \"Create's blocks, items and data for the Horizon port (behaviour is the native mod)\"\n\
authors = [\"The Create Team / The Creators of Create\"]\napi = \"0.2\"\nside = \"both\"\n\
capabilities = [\"registries\"]\n",
    );

    // The engine's table: every Create block, in registry order, as a `Block` enum.
    let mut rs = String::from(
        "// @generated by tools/create-data from Create 6.0.11's MIT data. Do not edit.\n\n\
         /// Every Create block, in the order Horizon registers them (sorted by id).\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]\n\
         #[repr(u16)]\npub enum Block {\n",
    );
    for b in &all {
        writeln!(rs, "    {},", camel(&b.name)).unwrap();
    }
    rs.push_str(
        "}\n\nimpl Block {\n    /// `create:<name>` without its namespace, by `Block as usize`.\n",
    );
    writeln!(rs, "    pub const NAMES: [&'static str; {}] = [", all.len()).unwrap();
    for b in &all {
        writeln!(rs, "        {:?},", b.name).unwrap();
    }
    rs.push_str("    ];\n\n    /// Stress impact per RPM (CStress.setImpact; 0 when Create sets none), by `Block as usize`.\n");
    writeln!(rs, "    pub const IMPACT: [f32; {}] = [", all.len()).unwrap();
    for b in &all {
        writeln!(rs, "        {:?},", b.java.impact.unwrap_or(0.0) as f32).unwrap();
    }
    rs.push_str("    ];\n\n    /// Stress capacity per RPM generated (CStress.setCapacity), by `Block as usize`.\n");
    writeln!(rs, "    pub const CAPACITY: [f32; {}] = [", all.len()).unwrap();
    for b in &all {
        writeln!(rs, "        {:?},", b.java.capacity.unwrap_or(0.0) as f32).unwrap();
    }
    rs.push_str("    ];\n\n    /// Every block, by `Block as usize`.\n");
    writeln!(rs, "    pub const ALL: [Block; {}] = [", all.len()).unwrap();
    for b in &all {
        writeln!(rs, "        Block::{},", camel(&b.name)).unwrap();
    }
    rs.push_str(
        "    ];\n\n    /// The block named `name` (without `create:`); a binary search, done once per state\n    \
         /// when a world opens, never in a tick.\n    \
         pub fn from_name(name: &str) -> Option<Block> {\n        \
         Self::NAMES.binary_search(&name).ok().map(|i| Self::ALL[i])\n    }\n\n    \
         pub fn name(self) -> &'static str {\n        Self::NAMES[self as usize]\n    }\n\n    \
         pub fn impact(self) -> f32 {\n        Self::IMPACT[self as usize]\n    }\n\n    \
         pub fn capacity(self) -> f32 {\n        Self::CAPACITY[self as usize]\n    }\n}\n",
    );
    let generated = out.join("mixins/create/engine/src/generated.rs");
    write(&generated, rs.as_bytes());
    // Keep the committed file in rustfmt's layout, so regenerating it changes nothing.
    let formatted = std::process::Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(&generated)
        .status();
    if !formatted.is_ok_and(|s| s.success()) {
        eprintln!("create-data: rustfmt failed on {}", generated.display());
    }

    println!(
        "{} Create blocks: {} declared, {} items declared",
        all.len(),
        declared.len(),
        items.len()
    );
    for (why, names) in &pending {
        println!(
            "pending ({}): {} [{}...]",
            names.len(),
            why,
            names[..names.len().min(4)].join(", ")
        );
    }
}

fn camel(name: &str) -> String {
    let mut out = String::new();
    for part in name.split('_') {
        let mut c = part.chars();
        if let Some(f) = c.next() {
            if out.is_empty() && f.is_ascii_digit() {
                out.push('N');
            }
            out.push(f.to_ascii_uppercase());
            out.extend(c);
        }
    }
    out
}
