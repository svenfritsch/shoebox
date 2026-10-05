//! Pets (phase 6): cats and dogs are found and matched like faces, with
//! their own model, so they are told apart from the people in everything
//! the user sees. This module holds what differs between the two kinds of
//! "face".
//!
//! - **Storage.** A pet is a row of `recog.faces` with `species` set to
//!   `cat` or `dog` (`NULL` for a person's face). Its box holds the whole
//!   pet, not just its head, and its embedding comes from another model,
//!   so it has another length and another scale. A pass over the photos of
//!   its own (`shoebox recognize --pets`, task `pets` in
//!   `recog.looked`) fills it; the faces pass never touches pet rows.
//! - **Spaces.** Embeddings of different models are never compared, so faces
//!   and pets are clustered and matched to people each in a space of
//!   their own (`clusters.rs`), with their own thresholds. A person (the
//!   user's entry with a name and a group) may be a pet: its confirmed
//!   examples are pets, and only pets are suggested for it.
//! - **Thresholds depend on the embedder.** ImageNet-style features have a
//!   high baseline (PP-ResNet50: different pets score 0.50–0.68, the same
//!   pet after brightness, blur and crop changes 0.95 and more), DINOv2's
//!   a lower one. The numbers are a first guess to be set from the pets
//!   check page on real photos (`docs/phase7.md`).

use crate::faces;
use crate::people;
use crate::recognize::{PETS, FACES};

/// The species the worker reports.
pub const SPECIES: [&str; 2] = ["cat", "dog"];

/// The species of a box the user drew around a pet: they say "a pet", not
/// which. A decision about it matches a detected cat or a dog (`kind_matches`).
pub const PET: &str = "pet";

/// Whether a decision about a face of `decision` species belongs to a detected
/// face of `face` species: a person's (`None`) to a person's, a cat's to a
/// cat, a drawn pet (`pet`) to any cat or dog.
pub fn kind_matches(decision: Option<&str>, face: Option<&str>) -> bool {
    match (decision, face) {
        (None, None) => true,
        (Some(d), Some(f)) => d == PET || d == f,
        _ => false,
    }
}

/// Pets narrower than this (px in the ≤1600 px copy the worker saw) are
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
    Pets,
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
        if species.is_some() { Space::Pets } else { Space::Faces }
    }

    /// The task in `recog.looked` that finds its rows.
    pub fn task(self) -> &'static str {
        match self {
            Space::Faces => FACES,
            Space::Pets => PETS,
        }
    }

    /// SQL for "a row of `recog.faces` (alias `f`) in this space".
    pub fn filter(self) -> &'static str {
        match self {
            Space::Faces => "f.species IS NULL",
            Space::Pets => "f.species IS NOT NULL",
        }
    }

    /// Narrowest box that takes part in clustering and suggestions, px.
    pub fn min_px(self) -> f64 {
        match self {
            Space::Faces => faces::MIN_CLUSTER_PX,
            Space::Pets => MIN_CLUSTER_PX,
        }
    }

    /// Whether a box this wide is too small to cluster or suggest.
    pub fn too_small(self, px: f64) -> bool {
        px < self.min_px()
    }

    /// The thresholds for embeddings of `model` (the worker's model id, for
    /// pets `<detector>+<embedder>`).
    pub fn thresholds(self, model: &str) -> Thresholds {
        match self {
            Space::Faces => {
                Thresholds { cluster: crate::clusters::CLUSTER_SIM, suggest: people::SUGGEST_SIM, maybe: people::MAYBE_SIM }
            }
            Space::Pets if model.ends_with("+dinov2-small") => Thresholds { cluster: 0.70, suggest: 0.60, maybe: 0.45 },
            // PP-ResNet50, and any embedder we do not know: strict, so
            // strangers are not joined (a new embedder starts safe, and the
            // check page shows where to set it).
            Space::Pets => Thresholds { cluster: 0.90, suggest: 0.85, maybe: 0.75 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_are_told_apart_by_species() {
        assert_eq!(Space::of(None), Space::Faces);
        assert_eq!(Space::of(Some("cat")), Space::Pets);
        assert_eq!(Space::Faces.task(), "faces");
        assert_eq!(Space::Pets.task(), "pets");
        assert!(is_species("dog") && !is_species("horse"));
    }

    #[test]
    fn faces_keep_their_calibrated_numbers() {
        let t = Space::Faces.thresholds("anything");
        assert_eq!((t.cluster, t.suggest, t.maybe), (0.60, 0.55, 0.35));
        assert!(Space::Faces.too_small(29.0) && !Space::Faces.too_small(30.0));
        assert!(Space::Pets.too_small(63.0) && !Space::Pets.too_small(64.0));
    }

    #[test]
    fn pets_need_much_stronger_matches() {
        for model in ["yolox-s-2022nov+ppresnet50-2022jan", "yolox-s-2022nov+dinov2-small", "something-new"] {
            let t = Space::Pets.thresholds(model);
            assert!(t.maybe < t.suggest && t.suggest < t.cluster, "{model}");
            assert!(t.maybe >= 0.45, "{model}: lower than the baseline of unrelated pets");
        }
        // Strangers score up to 0.68 with ResNet features: below every threshold.
        assert!(Space::Pets.thresholds("yolox-s-2022nov+ppresnet50-2022jan").maybe > 0.68);
    }

    #[test]
    fn a_drawn_pet_matches_any_pet_but_never_a_face() {
        assert!(kind_matches(None, None));
        assert!(kind_matches(Some("cat"), Some("cat")));
        assert!(!kind_matches(Some("cat"), Some("dog")));
        assert!(kind_matches(Some(PET), Some("cat")) && kind_matches(Some(PET), Some("dog")));
        assert!(!kind_matches(Some(PET), None) && !kind_matches(None, Some("dog")));
        assert!(!is_species(PET), "the worker never reports the generic species");
    }
}
