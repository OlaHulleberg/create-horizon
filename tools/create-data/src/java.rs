//! Facts read as text from Create's MIT Java registration chains (`REGISTRATE.block("name", ...)`
//! up to `.register()`): the block's class, its base properties and its sound. Nothing is run; a
//! chain built in a loop (palettes, colours) has no literal name and gets no facts.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JavaFacts {
    /// `ShaftBlock`, from `ShaftBlock::new`, `ShaftBlock::small` or `new ShaftBlock(`.
    pub class: Option<String>,
    /// `stone`, `softMetal`, `wooden`, `copperMetal`, `netheriteMetal` (`SharedProperties::x`),
    /// or a vanilla block (`Blocks.GLASS` gives `glass`).
    pub base: Option<String>,
    /// `SoundType.X`, lowercased: `metal`, `wood`, `netherite_block`...
    pub sound: Option<String>,
    pub no_occlusion: bool,
    /// An item chain that sets `.food(...)`.
    pub food: bool,
    /// `CStress.setImpact(x)` (`setNoImpact` is 0), directly or through a `BuilderTransformers`
    /// helper that sets it: stress units per RPM.
    pub impact: Option<f64>,
    /// `CStress.setCapacity(x)`: stress units per RPM of generated speed.
    pub capacity: Option<f64>,
}

/// `BuilderTransformers` helpers that set a stress impact (BuilderTransformers.java).
const TRANSFORMER_IMPACTS: [(&str, f64); 3] = [
    ("BuilderTransformers.cuckooClock(", 1.0),
    ("BuilderTransformers.mechanicalPiston(", 4.0),
    ("BuilderTransformers.backtank(", 4.0),
];

fn number_after(s: &str, prefix: &str) -> Option<f64> {
    let i = s.find(prefix)? + prefix.len();
    let t = &s[i..];
    let n = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    t[..n].parse().ok()
}

/// Item registrations: `REGISTRATE.item("name", X::new)` chains, and Create's `ingredient("name")`
/// and `taggedIngredient("name", ...)` helpers, which register plain `Item`s.
pub fn read_items(java_root: &Path) -> BTreeMap<String, JavaFacts> {
    let mut out = BTreeMap::new();
    let mut files = Vec::new();
    collect(java_root, &mut files);
    files.sort();
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for (name, mut facts) in chains(&text, ".item(\"") {
            facts.food = facts.food || text_has_food(&text, &name);
            out.entry(name).or_insert(facts);
        }
        for marker in ["= ingredient(\"", " ingredient(\"", "taggedIngredient(\""] {
            let mut rest = text.as_str();
            while let Some(at) = rest.find(marker) {
                let after = &rest[at + marker.len()..];
                let Some(end) = after.find('"') else { break };
                out.entry(after[..end].to_string()).or_insert(JavaFacts {
                    class: Some("Item".into()),
                    ..JavaFacts::default()
                });
                rest = &after[end..];
            }
        }
    }
    out
}

fn text_has_food(text: &str, name: &str) -> bool {
    let Some(at) = text.find(&format!(".item(\"{name}\"")) else {
        return false;
    };
    let chain = &text[at..];
    let end = chain.find(".register()").unwrap_or(chain.len());
    chain[..end].contains(".food(")
}

pub fn read_all(java_root: &Path) -> BTreeMap<String, JavaFacts> {
    let mut out = BTreeMap::new();
    let mut files = Vec::new();
    collect(java_root, &mut files);
    files.sort();
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for (name, facts) in chains(&text, ".block(\"") {
            out.entry(name).or_insert(facts);
        }
    }
    out
}

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "java") {
            out.push(p);
        }
    }
}

/// Every `.block("literal", ...)` chain in one file.
pub fn chains(text: &str, marker: &str) -> Vec<(String, JavaFacts)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(marker) {
        let after = &rest[at + marker.len()..];
        let Some(end_name) = after.find('"') else {
            break;
        };
        let name = &after[..end_name];
        let body_end = after.find(".register()").unwrap_or(after.len());
        let body = &after[end_name..body_end];
        if name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            out.push((name.to_string(), facts(body)));
        }
        rest = &after[end_name..];
    }
    out
}

fn ident_after<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let i = s.find(prefix)? + prefix.len();
    let t = &s[i..];
    let n = t
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(t.len());
    (n > 0).then(|| &t[..n])
}

fn facts(body: &str) -> JavaFacts {
    // The constructor is the second argument of `.block(name, X::new)` or a lambda `new X(`.
    let head = &body[..body
        .find(')')
        .map(|i| i + 1)
        .unwrap_or(body.len())
        .min(body.len())];
    // A lambda `p -> new X(p, ...)` names the class; otherwise the `X::new` reference does.
    let class = ident_after(head, "new ").map(str::to_string).or_else(|| {
        head.split([',', ' ', '(', '>']).find_map(|tok| {
            tok.trim()
                .split_once("::")
                .map(|(c, _)| c.to_string())
                .filter(|c| c.chars().next().is_some_and(char::is_uppercase))
        })
    });
    let base = ident_after(body, "initialProperties(SharedProperties::")
        .map(str::to_string)
        .or_else(|| {
            ident_after(body, "initialProperties(() -> Blocks.").map(|s| s.to_ascii_lowercase())
        });
    let sound = ident_after(body, "SoundType.").map(|s| s.to_ascii_lowercase());
    JavaFacts {
        class,
        base,
        sound,
        no_occlusion: body.contains("noOcclusion()"),
        food: body.contains(".food("),
        impact: number_after(body, "CStress.setImpact(")
            .or_else(|| body.contains("CStress.setNoImpact()").then_some(0.0))
            .or_else(|| {
                TRANSFORMER_IMPACTS
                    .iter()
                    .find(|(t, _)| body.contains(t))
                    .map(|(_, v)| *v)
            }),
        capacity: number_after(body, "CStress.setCapacity("),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_registration_chain() {
        let java = r#"
	public static final BlockEntry<CogWheelBlock> COGWHEEL = REGISTRATE.block("cogwheel", CogWheelBlock::small)
		.initialProperties(SharedProperties::stone)
		.properties(p -> p.sound(SoundType.WOOD)
			.mapColor(MapColor.DIRT))
		.transform(CStress.setNoImpact())
		.register();
	public static final BlockEntry<CreativeMotorBlock> CREATIVE_MOTOR = REGISTRATE.block("creative_motor", CreativeMotorBlock::new)
		.transform(CStress.setCapacity(16384.0))
		.register();
	public static final BlockEntry<EncasedShaftBlock> X =
		REGISTRATE.block("andesite_encased_shaft", p -> new EncasedShaftBlock(p, AllBlocks.ANDESITE_CASING::get))
			.register();
"#;
        let c = chains(java, ".block(\"");
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].1.impact, Some(0.0));
        assert_eq!(c[1].1.capacity, Some(16384.0));
        assert_eq!(c[0].0, "cogwheel");
        assert_eq!(c[0].1.class.as_deref(), Some("CogWheelBlock"));
        assert_eq!(c[0].1.base.as_deref(), Some("stone"));
        assert_eq!(c[0].1.sound.as_deref(), Some("wood"));
        assert_eq!(c[2].1.class.as_deref(), Some("EncasedShaftBlock"));
    }
}
