//! Differences confirmed not to be port bugs, each with its evidence. The comparison skips only
//! the exact pattern described.

/// Exact nearest-direction tie evidence, propagated only through placement/wrench
/// orientation and the tied chain component's computed shape properties.
/// Java 1.21.1: yaw 45, pitch 0, radians -0.7853982, sin bits 3207922931,
/// cos bits 1060439283 (equal magnitudes), nearest SOUTH. Port cosine bits
/// 1060438146 select WEST. No approximate angles or horizontal-direction APIs qualify.
///
/// Not a port bug: the tie is decided by vanilla's `Mth` sine table, not by Create's code. The
/// port's `place::mth_sin`/`mth_cos` (`create/src/engine/src/place.rs`) use the same table
/// and indexing as the game it runs in: Sunrise's `sunrise-core/src/geometry/trig.rs`
/// (`SCALE = 10430.378350470453`, entries `(i / SCALE).sin() as f32`, double-to-long index,
/// "generated from the 26.3 table formula"), and Sunrise's own `level/place.rs`
/// `ordered_by_nearest` reads it. The Java host runs Minecraft 1.21.1, whose table ties at
/// this angle (bits measured above). Matching 1.21.1 here would make the port disagree with
/// the game it ships in, so the difference is between vanilla versions, not a port bug.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NearestTie {
    Yaw45Pitch0,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MeasuredTrigState {
    #[serde(default)]
    pub tie: Option<NearestTie>,
    pub java: String,
    pub port: String,
    #[serde(default)]
    pub properties: Vec<String>,
}

pub fn trig_state(java: &str, port: &str, measured: Option<&MeasuredTrigState>) -> bool {
    let normalize = |s: &str| {
        let (name, props) = crate::scenario::parse_state(s);
        crate::scenario::format_state(&name, &props)
    };
    measured.is_some_and(|m| {
        let (jn, jp) = crate::scenario::parse_state(&m.java);
        let (rn, rp) = crate::scenario::parse_state(&m.port);
        m.tie == Some(NearestTie::Yaw45Pitch0)
            && jn == rn
            && !m.properties.is_empty()
            && m.properties
                .iter()
                .all(|p| ["axis", "facing", "part", "axis_along_first"].contains(&p.as_str()))
            && jp.len() == rp.len()
            && jp.iter().all(|(k, v)| {
                rp.iter()
                    .any(|(rk, rv)| rk == k && (rv == v || m.properties.contains(k)))
            })
            && normalize(java) == normalize(&m.java)
            && normalize(port) == normalize(&m.port)
    })
}

#[cfg(test)]
mod measured_tests {
    #[test]
    fn only_measured_state_pair_is_allowed() {
        let m = super::MeasuredTrigState {
            tie: Some(super::NearestTie::Yaw45Pitch0),
            properties: vec!["axis".into()],
            java: "create:shaft[axis=z,waterlogged=false]".into(),
            port: "create:shaft[axis=x,waterlogged=false]".into(),
        };
        assert!(super::trig_state(&m.java, &m.port, Some(&m)));
        assert!(!super::trig_state(
            "create:shaft[axis=y,waterlogged=false]",
            &m.port,
            Some(&m)
        ));
        assert!(!super::trig_state(&m.java, &m.port, None));
        let sc = serde_json::from_str(
            r#"{
  "version": 3,
  "ticks": 140,
  "blocks": [
    {
      "pos": [
        2,
        7,
        0
      ],
      "state": "minecraft:stone"
    }
  ],
  "checkpoints": [
    10,
    73
  ],
  "events": [
    {
      "tick": 4,
      "pos": [
        2,
        7,
        0
      ],
      "action": {
        "kind": "use",
        "item": "create:shaft",
        "face": "up",
        "hit": [
          0.5,
          1.0,
          0.5
        ],
        "yaw": 45.0,
        "pitch": 0.0,
        "sneaking": false
      }
    },
    {
      "tick": 70,
      "pos": [
        2,
        8,
        0
      ],
      "action": {
        "kind": "use",
        "item": "create:wrench",
        "face": "up",
        "hit": [
          0.5,
          1.0,
          0.5
        ],
        "yaw": 0.0,
        "pitch": 0.0,
        "sneaking": false
      }
    }
  ]
}
"#,
        )
        .unwrap();
        let snap = crate::engine::run(&sc);
        let r = &snap.blocks["2,8,0"];
        let m = r.measured_trig.as_ref().unwrap();
        assert_eq!(m.java, "create:shaft[axis=x,waterlogged=false]");
        assert_eq!(m.port, "create:shaft[axis=z,waterlogged=false]");
        let mut j = snap.clone();
        j.blocks.get_mut("2,8,0").unwrap().state = m.java.clone();
        assert!(crate::compare::diff(&j, &snap).is_empty());
        j.networks
            .as_mut()
            .unwrap()
            .sizes
            .insert("2,8,0".into(), 99);
        assert!(crate::compare::diff(&j, &snap).contains(&"network sizes differ".into()));
    }
}
