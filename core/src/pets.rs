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

/// What the user can search for: a species (`cat`, `dog`) or any pet (`pet`),
/// in English and German, as typed words and in the suggestions.
const SEARCH_TERMS: [(&str, &[&str]); 3] = [
    ("cat", &["cat", "cats", "katze", "katzen", "kater"]),
    ("dog", &["dog", "dogs", "hund", "hunde"]),
    ("pet", &["pet", "pets", "haustier", "haustiere", "tier", "tiere"]),
];

/// Whether `s` is a species a search can ask for (`cat`, `dog`, `pet`).
pub fn is_search_species(s: &str) -> bool {
    SEARCH_TERMS.iter().any(|(species, _)| *species == s)
}

/// The search terms a typed word means: at least three letters that start one
/// of the words for it ("kat" and "katzen" mean cats, "pets" any pet; "ca"
/// means nothing, so it does not match pets for every word with those letters).
pub fn species_for_word(word: &str) -> Vec<&'static str> {
    let word = word.to_lowercase();
    if word.chars().count() < 3 {
        return Vec::new();
    }
    SEARCH_TERMS.iter().filter(|(_, aliases)| aliases.iter().any(|a| a.starts_with(&word))).map(|(s, _)| *s).collect()
}

/// The search terms a suggestion box offers for what is typed so far (all of
/// them for nothing typed): any word for the term that contains it.
pub fn species_matching(needle: &str) -> Vec<&'static str> {
    let needle = needle.trim().to_lowercase();
    SEARCH_TERMS
        .iter()
        .filter(|(_, aliases)| needle.is_empty() || aliases.iter().any(|a| a.contains(&needle)))
        .map(|(s, _)| *s)
        .collect()
}

/// A "face" counts as a pet's face when at least this much of it lies inside
/// a pet's box (the detector for people's faces also fires on cats and dogs).
pub const PET_FACE_INSIDE: f64 = 0.7;
/// … unless at least this much of it lies inside a person's box: a face on a
/// person (a child hugging a dog) stays a face. Where no person was found
/// the face is taken for the pet's.
pub const PERSON_FACE_INSIDE: f64 = 0.5;

/// How much of box `a` (x, y, w, h) lies inside box `b`, 0 to 1.
pub fn share_inside(a: [f64; 4], b: [f64; 4]) -> f64 {
    let ix = ((a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0])).max(0.0);
    let iy = ((a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1])).max(0.0);
    let area = a[2] * a[3];
    if area > 0.0 { ix * iy / area } else { 0.0 }
}

/// Whether a detected "face" is really a pet's face: mostly inside a pet's
/// box and not inside a person's. Decided from the boxes of one photo (all
/// fractions of the picture); the pets must be the ones still counted (not
/// marked "not a pet"), and the face one nobody has decided on.
pub fn is_pet_face(face: [f64; 4], pets: &[[f64; 4]], people: &[[f64; 4]]) -> bool {
    pets.iter().any(|&p| share_inside(face, p) >= PET_FACE_INSIDE)
        && !people.iter().any(|&b| share_inside(face, b) >= PERSON_FACE_INSIDE)
}

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
    fn typed_words_mean_a_species_in_english_and_german() {
        assert_eq!(species_for_word("cat"), ["cat"]);
        assert_eq!(species_for_word("Katze"), ["cat"]);
        assert_eq!(species_for_word("kat"), ["cat"]);
        assert_eq!(species_for_word("Hunde"), ["dog"]);
        assert_eq!(species_for_word("dog"), ["dog"]);
        assert_eq!(species_for_word("pets"), ["pet"]);
        assert_eq!(species_for_word("Haustier"), ["pet"]);
        // Too short, or only the start of another word.
        assert!(species_for_word("ca").is_empty() && species_for_word("do").is_empty());
        assert!(species_for_word("catalog").is_empty() && species_for_word("hundert").is_empty() && species_for_word("petra").is_empty());
        assert_eq!(species_matching("").len(), 3);
        assert_eq!(species_matching("ka"), ["cat"]);
        assert_eq!(species_matching("hu"), ["dog"]);
        assert_eq!(species_matching("t"), ["cat", "pet"], "cat, katze, tier ... contain a t");
        assert!(species_matching("xyz").is_empty());
        assert!(is_search_species("pet") && is_search_species("cat") && !is_search_species("horse"));
    }

    #[test]
    fn a_face_in_a_pet_and_outside_every_person_is_the_pets() {
        let dog = [0.1, 0.5, 0.3, 0.4];
        let dogs_face = [0.15, 0.52, 0.1, 0.1];
        let woman = [0.5, 0.1, 0.3, 0.8];
        let womans_face = [0.58, 0.12, 0.1, 0.1];
        // The screenshot case: the dog's face, a woman standing elsewhere.
        assert!(is_pet_face(dogs_face, &[dog], &[woman]));
        assert!(is_pet_face(dogs_face, &[dog], &[]), "no person found: the pet's");
        // The woman's own face is no pet's, with or without a pet in the photo.
        assert!(!is_pet_face(womans_face, &[dog], &[woman]));
        assert!(!is_pet_face(womans_face, &[], &[woman]));
        // A child lying on a big dog: the face is inside the dog's box, but also in the child's.
        let big_dog = [0.0, 0.2, 1.0, 0.7];
        let child = [0.4, 0.3, 0.3, 0.3];
        assert!(!is_pet_face([0.45, 0.32, 0.1, 0.1], &[big_dog], &[child]));
        // Only partly inside the pet (a person standing beside it): not the pet's.
        assert!(!is_pet_face([0.35, 0.52, 0.1, 0.1], &[dog], &[]));
        assert!((share_inside([0.0, 0.0, 0.2, 0.2], [0.1, 0.0, 0.2, 0.2]) - 0.5).abs() < 1e-9);
        assert_eq!(share_inside([0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 1.0]), 0.0);
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
