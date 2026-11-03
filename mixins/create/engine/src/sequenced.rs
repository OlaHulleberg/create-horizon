//! Sequenced assembly (`SequencedAssemblyRecipe`, `SequencedRecipe`): an item goes through a
//! list of processing steps (pressing, deploying, filling, cutting) a number of loops; between
//! steps it is the recipe's transitional item, carrying the recipe and the step it is at
//! (`SequencedAssembly`), and after the last it becomes one of the results, by weight. Each
//! machine asks for the step an item is at before its own recipes (`getRecipe`).

/// One step: the machine's recipe type, and what else it takes (a deployer's held item, a
/// spout's fluid), with whether a deployer keeps its item.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub kind: String,
    /// The items the step's second ingredient accepts (a deployer's), if it has one.
    pub with: Option<Vec<String>>,
    /// The fluid and amount a filling step takes.
    pub fluid: Option<(String, u32)>,
    pub keep_held: bool,
    /// Its `processing_time` (a saw's).
    pub time: i32,
}

/// A result of the pool, its count and weight (`chance`).
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub item: String,
    pub count: u8,
    pub weight: f32,
}

/// A `create:sequenced_assembly` recipe.
#[derive(Clone, Debug, PartialEq)]
pub struct Assembly {
    pub id: String,
    /// The items its first step takes besides the transitional item (`ingredient`).
    pub ingredient: Vec<String>,
    pub transitional: String,
    pub steps: Vec<Step>,
    pub loops: u32,
    pub results: Vec<Outcome>,
}

/// What an item says of the assembly it is in (`SequencedAssembly`: the recipe, its step).
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub id: String,
    pub step: u32,
}

/// What a step turns its item into.
#[derive(Clone, Debug, PartialEq)]
pub enum Advanced {
    /// The transitional item, a step further (`progress` is the step over all steps).
    Next {
        item: String,
        progress: Progress,
        fraction: f32,
    },
    /// The last step done: a result of the pool.
    Done { item: String, count: u8 },
    /// The pool rolled nothing.
    Nothing,
}

impl Assembly {
    /// `appliesTo`: an item in the middle of this assembly (its transitional item with its
    /// progress), or one its first step takes.
    pub fn applies_to(&self, item: &str, progress: Option<&Progress>) -> bool {
        match progress {
            Some(p) => self.transitional == item && p.id == self.id,
            None => self.ingredient.iter().any(|i| i == item),
        }
    }

    /// `getNextRecipe`: the step the item is at.
    pub fn next_step(&self, progress: Option<&Progress>) -> Option<&Step> {
        if self.steps.is_empty() {
            return None;
        }
        let step = progress.map_or(0, |p| p.step) as usize;
        self.steps.get(step % self.steps.len())
    }

    /// `advance`: after its step the item is the transitional item a step on, or, past its last
    /// loop, a result rolled from the pool by weight (`rollResult`; `roll` in 0..1).
    pub fn advance(&self, progress: Option<&Progress>, roll: f32) -> Advanced {
        let step = progress.map_or(0, |p| p.step);
        let len = self.steps.len().max(1) as u32;
        if (step + 1) / len >= self.loops {
            let total: f32 = self.results.iter().map(|r| r.weight).sum();
            let mut number = roll * total;
            for r in &self.results {
                number -= r.weight;
                if number < 0.0 {
                    return Advanced::Done {
                        item: r.item.clone(),
                        count: r.count,
                    };
                }
            }
            return Advanced::Nothing;
        }
        Advanced::Next {
            item: self.transitional.clone(),
            progress: Progress {
                id: self.id.clone(),
                step: step + 1,
            },
            fraction: (step + 1) as f32 / (len * self.loops) as f32,
        }
    }
}

/// `SequencedAssemblyRecipe.getRecipe`: the first assembly the item applies to whose step it is
/// at is of machine type `kind`, with that step.
pub fn step_for<'a>(
    all: &'a [Assembly],
    item: &str,
    progress: Option<&Progress>,
    kind: &str,
) -> Option<(&'a Assembly, &'a Step)> {
    all.iter().find_map(|a| {
        if !a.applies_to(item, progress) {
            return None;
        }
        let s = a.next_step(progress)?;
        (s.kind == kind).then_some((a, s))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mechanism() -> Assembly {
        // Create's precision mechanism, shortened: deploy a cog, press, five loops.
        Assembly {
            id: "create:sequenced_assembly/precision_mechanism".into(),
            ingredient: vec!["create:golden_sheet".into()],
            transitional: "create:incomplete_precision_mechanism".into(),
            steps: vec![
                Step {
                    kind: "create:deploying".into(),
                    with: Some(vec!["create:cogwheel".into()]),
                    fluid: None,
                    keep_held: false,
                    time: 0,
                },
                Step {
                    kind: "create:pressing".into(),
                    with: None,
                    fluid: None,
                    keep_held: false,
                    time: 0,
                },
            ],
            loops: 5,
            results: vec![
                Outcome {
                    item: "create:precision_mechanism".into(),
                    count: 1,
                    weight: 120.0,
                },
                Outcome {
                    item: "create:golden_sheet".into(),
                    count: 1,
                    weight: 8.0,
                },
            ],
        }
    }

    #[test]
    fn an_item_steps_through_its_sequence_and_loops() {
        let all = vec![mechanism()];
        // A golden sheet: the first step, a deployer's; a press asks in vain.
        assert!(step_for(&all, "create:golden_sheet", None, "create:pressing").is_none());
        let (a, s) = step_for(&all, "create:golden_sheet", None, "create:deploying").unwrap();
        assert_eq!(
            s.with.as_deref(),
            Some(&["create:cogwheel".to_string()][..])
        );
        let Advanced::Next {
            item,
            progress,
            fraction,
        } = a.advance(None, 0.0)
        else {
            panic!("a step on");
        };
        assert_eq!(
            (item.as_str(), progress.step, fraction),
            ("create:incomplete_precision_mechanism", 1, 0.1)
        );
        // Now the press's; the deployer's again after it.
        let p = Some(&progress);
        assert!(step_for(&all, &item, p, "create:pressing").is_some());
        assert!(step_for(&all, &item, p, "create:deploying").is_none());
        // A transitional item of another assembly, or without progress, is nobody's.
        let other = Progress {
            id: "x".into(),
            step: 1,
        };
        assert!(step_for(&all, &item, Some(&other), "create:pressing").is_none());
        assert!(step_for(&all, &item, None, "create:pressing").is_none());
    }

    #[test]
    fn the_last_step_rolls_the_pool() {
        let a = mechanism();
        let last = Progress {
            id: a.id.clone(),
            step: 9,
        };
        // 2 steps × 5 loops: step 9's advance finishes; 120 to 8 by weight.
        assert_eq!(
            a.advance(Some(&last), 0.5),
            Advanced::Done {
                item: "create:precision_mechanism".into(),
                count: 1
            }
        );
        assert_eq!(
            a.advance(Some(&last), 0.99),
            Advanced::Done {
                item: "create:golden_sheet".into(),
                count: 1
            }
        );
        let eighth = Progress {
            id: a.id.clone(),
            step: 8,
        };
        assert!(matches!(
            a.advance(Some(&eighth), 0.5),
            Advanced::Next { .. }
        ));
    }
}
