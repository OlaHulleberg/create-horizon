//! Screen packet payload, independent of engine storage.
use create_engine::sequencer;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Kind {
    TurnAngle,
    TurnDistance,
    Delay,
    Await,
    End,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Modifier {
    ForwardFast,
    Forward,
    Back,
    BackFast,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    pub instruction: Kind,
    pub modifier: Modifier,
    pub value: i32,
}
impl Instruction {
    pub fn validate(&self) {
        let max = match self.instruction {
            Kind::TurnAngle => 360,
            Kind::TurnDistance => 128,
            Kind::Delay => 600,
            Kind::Await | Kind::End => return,
        };
        assert!(
            (1..=max).contains(&self.value),
            "instruction parameter outside screen range"
        );
    }
    pub fn engine(&self) -> sequencer::Instruction {
        sequencer::Instruction {
            kind: match self.instruction {
                Kind::TurnAngle => sequencer::Kind::TurnAngle,
                Kind::TurnDistance => sequencer::Kind::TurnDistance,
                Kind::Delay => sequencer::Kind::Delay,
                Kind::Await => sequencer::Kind::Await,
                Kind::End => sequencer::Kind::End,
            },
            speed: match self.modifier {
                Modifier::ForwardFast => sequencer::Speed::ForwardFast,
                Modifier::Forward => sequencer::Speed::Forward,
                Modifier::Back => sequencer::Speed::Back,
                Modifier::BackFast => sequencer::Speed::BackFast,
            },
            value: self.value,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn packet_rows_validate_and_keep_modifiers() {
        for modifier in [
            super::Modifier::ForwardFast,
            super::Modifier::Forward,
            super::Modifier::Back,
            super::Modifier::BackFast,
        ] {
            let i = super::Instruction {
                instruction: super::Kind::TurnDistance,
                modifier,
                value: 128,
            };
            i.validate();
            assert_eq!(i.engine().value, 128);
            let copy: super::Instruction =
                serde_json::from_str(&serde_json::to_string(&i).unwrap()).unwrap();
            assert_eq!(copy, i);
        }
        assert!(
            serde_json::from_str::<super::Instruction>(
                r#"{"instruction":"TYPO","modifier":"FORWARD","value":1}"#
            )
            .is_err()
        );
    }
}
