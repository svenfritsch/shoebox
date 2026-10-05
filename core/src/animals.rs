//! Pets (phase 6): cats and dogs are found and matched like faces, with
//! their own model, so they are told apart from the people in everything
//! the user sees. This module holds what differs between the two kinds of
//! "face".
//!
//! - **Storage.** An animal is a row of `recog.faces` with `species` set to
//!   `cat` or `dog` (`NULL` for a person's face). Its box holds the whole
//!   animal, not just its head, and its embedding comes from another model,
//!   so it has another length and another scale. A pass over the photos of
//!   its own (`shoebox recognize --animals`, task `animals` in
//!   `recog.looked`) fills it; the faces pass never touches animal rows.
//! - **Spaces.** Embeddings of different models are never compared, so faces
//!   and animals are clustered and matched to people each in a space of
//!   their own (`clusters.rs`), with their own thresholds. A person (the
//!   user's entry with a name and a group) may be a pet: its confirmed
//!   examples are animals, and only animals are suggested for it.
//! - **Thresholds depend on the embedder.** ImageNet-style features have a
//!   high baseline (PP-ResNet50: different animals score 0.50–0.68, the same
//!   animal after brightness, blur and crop changes 0.95 and more), DINOv2's
//!   a lower one. The numbers are a first guess to be set from the animals
//!   check page on real photos (`docs/phase6.md`).

use crate::faces;
use crate::people;
use crate::recognize::{ANIMALS, FACES};

/// The species the worker reports.
pub const SPECIES: [&str; 2] = ["cat", "dog"];

/// Animals narrower than this (px in the ≤1600 px copy the worker saw) are
/// listed but too small to cluster or suggest. The worker drops boxes under
/// 48 px altogether.
pub const MIN_CLUSTER_PX: f64 = 64.0;

pub fn is_species(s: &str) -> bool {
    SPECIES.contains(&s)
}

/// Which kind of face: a person's, or a cat's or dog's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    Faces,
    Animals,
}

/// The similarities (cosine) that matter in one space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// At least this similar: neighbours, and so one cluster.
    pub cluster: f32,
    /// At least this similar to a confirmed face of a person: suggested.
    pub suggest: f32,
    /// Between this and `suggest`: only offered as "maybe".
    pub maybe: f32,
}

impl Space {
    /// The space a row of `recog.faces` is in, from its `species` column.
    pub fn of(species: Option<&str>) -> Space {
        if species.is_some() { Space::Animals } else { Space::Faces }
    }

    /// The task in `recog.looked` that finds its rows.
    pub fn task(self) -> &'static str {
        match self {
            Space::Faces => FACES,
            Space::Animals => ANIMALS,
        }
    }

    /// SQL for "a row of `recog.faces` (alias `f`) in this space".
    pub fn filter(self) -> &'static str {
        match self {
            Space::Faces => "f.species IS NULL",
            Space::Animals => "f.species IS NOT NULL",
        }
    }

    /// Narrowest box that takes part in clustering and suggestions, px.
    pub fn min_px(self) -> f64 {
        match self {
            Space::Faces => faces::MIN_CLUSTER_PX,
            Space::Animals => MIN_CLUSTER_PX,
        }
    }

    /// Whether a box this wide is too small to cluster or suggest.
    pub fn too_small(self, px: f64) -> bool {
        px < self.min_px()
    }

    /// The thresholds for embeddings of `model` (the worker's model id, for
    /// animals `<detector>+<embedder>`).
    pub fn thresholds(self, model: &str) -> Thresholds {
        match self {
            Space::Faces => {
                Thresholds { cluster: crate::clusters::CLUSTER_SIM, suggest: people::SUGGEST_SIM, maybe: people::MAYBE_SIM }
            }
            Space::Animals if model.ends_with("+dinov2-small") => Thresholds { cluster: 0.70, suggest: 0.60, maybe: 0.45 },
            // PP-ResNet50, and any embedder we do not know: strict, so
            // strangers are not joined (a new embedder starts safe, and the
            // check page shows where to set it).
            Space::Animals => Thresholds { cluster: 0.90, suggest: 0.85, maybe: 0.75 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_are_told_apart_by_species() {
        assert_eq!(Space::of(None), Space::Faces);
        assert_eq!(Space::of(Some("cat")), Space::Animals);
        assert_eq!(Space::Faces.task(), "faces");
        assert_eq!(Space::Animals.task(), "animals");
        assert!(is_species("dog") && !is_species("horse"));
    }

    #[test]
    fn faces_keep_their_calibrated_numbers() {
        let t = Space::Faces.thresholds("anything");
        assert_eq!((t.cluster, t.suggest, t.maybe), (0.60, 0.55, 0.35));
        assert!(Space::Faces.too_small(29.0) && !Space::Faces.too_small(30.0));
        assert!(Space::Animals.too_small(63.0) && !Space::Animals.too_small(64.0));
    }

    #[test]
    fn animals_need_much_stronger_matches() {
        for model in ["yolox-s-2022nov+ppresnet50-2022jan", "yolox-s-2022nov+dinov2-small", "something-new"] {
            let t = Space::Animals.thresholds(model);
            assert!(t.maybe < t.suggest && t.suggest < t.cluster, "{model}");
            assert!(t.maybe >= 0.45, "{model}: lower than the baseline of unrelated animals");
        }
        // Strangers score up to 0.68 with ResNet features: below every threshold.
        assert!(Space::Animals.thresholds("yolox-s-2022nov+ppresnet50-2022jan").maybe > 0.68);
    }
}
