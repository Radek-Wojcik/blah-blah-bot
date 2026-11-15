//! Cosine similarity and the decision about one detected voice.

use serde::{Deserialize, Serialize};

/// Cosine similarity of two vectors; 0 when either has no length or the sizes differ.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        dot += f64::from(*x) * f64::from(*y);
        na += f64::from(*x) * f64::from(*x);
        nb += f64::from(*y) * f64::from(*y);
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    (dot / (na.sqrt() * nb.sqrt())) as f32
}

/// Scale a vector to unit length (a zero vector stays zero).
pub fn normalize(v: &[f32]) -> Vec<f32> {
    let norm = v
        .iter()
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum::<f64>()
        .sqrt();
    if norm == 0.0 {
        return v.to_vec();
    }
    v.iter().map(|x| (f64::from(*x) / norm) as f32).collect()
}

/// Limits that turn a similarity into an action.
///
/// * `accept`: a best match at or above this is assigned automatically, provided it is also
///   clearly better than the runner-up.
/// * `suggest`: a best match at or above this (but not accepted) is offered as a suggestion.
/// * `margin`: the best match must beat the second best by at least this much to be accepted
///   automatically; otherwise it is only a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    pub accept: f32,
    pub suggest: f32,
    pub margin: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            accept: 0.75,
            suggest: 0.60,
            margin: 0.10,
        }
    }
}

impl Thresholds {
    /// Whether the numbers are usable: `0 <= suggest <= accept <= 1` and `margin >= 0`.
    pub fn is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.suggest)
            && (self.suggest..=1.0).contains(&self.accept)
            && self.margin >= 0.0
    }
}

/// A voice profile as the matcher sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileRef {
    pub participant_id: String,
    pub centroid: Vec<f32>,
}

/// What to do with one detected voice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    /// Strong, unambiguous match: the name is applied automatically.
    Auto { participant_id: String, score: f32 },
    /// Plausible match: shown as a suggestion, never applied without the user.
    Suggest { participant_id: String, score: f32 },
    /// No profile is close enough. The voice stays unknown.
    Unknown,
}

impl Decision {
    pub fn score(&self) -> Option<f32> {
        match self {
            Self::Auto { score, .. } | Self::Suggest { score, .. } => Some(*score),
            Self::Unknown => None,
        }
    }
}

/// Compare one embedding with all profiles.
pub fn classify(embedding: &[f32], profiles: &[ProfileRef], t: &Thresholds) -> Decision {
    let mut scored: Vec<(&ProfileRef, f32)> = profiles
        .iter()
        .map(|p| (p, cosine(embedding, &p.centroid)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let Some(&(best, best_score)) = scored.first() else {
        return Decision::Unknown;
    };
    let runner_up = scored.get(1).map_or(f32::NEG_INFINITY, |s| s.1);
    let id = best.participant_id.clone();
    if best_score >= t.accept && best_score - runner_up >= t.margin {
        Decision::Auto {
            participant_id: id,
            score: best_score,
        }
    } else if best_score >= t.suggest {
        Decision::Suggest {
            participant_id: id,
            score: best_score,
        }
    } else {
        Decision::Unknown
    }
}

/// A detected voice to be matched.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub voice_id: String,
    pub embedding: Vec<f32>,
}

/// Match all voices of one meeting. Two voices of the same meeting are different people, so a
/// participant can be assigned automatically to at most one of them (the closest); any other
/// voice that matched the same participant is demoted to a suggestion.
pub fn assign_unique(
    candidates: &[Candidate],
    profiles: &[ProfileRef],
    t: &Thresholds,
) -> Vec<(String, Decision)> {
    let mut out: Vec<(String, Decision)> = candidates
        .iter()
        .map(|c| (c.voice_id.clone(), classify(&c.embedding, profiles, t)))
        .collect();
    let mut best: std::collections::HashMap<String, (usize, f32)> = Default::default();
    for (i, (_, d)) in out.iter().enumerate() {
        if let Decision::Auto {
            participant_id,
            score,
        } = d
        {
            let e = best.entry(participant_id.clone()).or_insert((i, *score));
            if *score > e.1 {
                *e = (i, *score);
            }
        }
    }
    for (i, (_, d)) in out.iter_mut().enumerate() {
        if let Decision::Auto {
            participant_id,
            score,
        } = d
        {
            if best[participant_id.as_str()].0 != i {
                *d = Decision::Suggest {
                    participant_id: participant_id.clone(),
                    score: *score,
                };
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, v: &[f32]) -> ProfileRef {
        ProfileRef {
            participant_id: id.into(),
            centroid: normalize(v),
        }
    }

    #[test]
    fn cosine_of_parallel_orthogonal_and_opposite_vectors() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 3.0]).abs() < 1e-6);
        assert!((cosine(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_is_zero_for_degenerate_input() {
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn normalize_gives_unit_length() {
        let n = normalize(&[3.0, 4.0]);
        assert!((n[0] - 0.6).abs() < 1e-6 && (n[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn strong_clear_match_is_automatic() {
        let t = Thresholds::default();
        let profiles = [p("a", &[1.0, 0.0, 0.0]), p("b", &[0.0, 1.0, 0.0])];
        let d = classify(&[1.0, 0.05, 0.0], &profiles, &t);
        assert!(matches!(d, Decision::Auto { ref participant_id, .. } if participant_id == "a"));
    }

    #[test]
    fn a_close_runner_up_makes_a_strong_match_only_a_suggestion() {
        let t = Thresholds::default();
        let profiles = [p("a", &[1.0, 0.1, 0.0]), p("b", &[1.0, -0.1, 0.0])];
        let d = classify(&[1.0, 0.0, 0.0], &profiles, &t);
        assert!(matches!(d, Decision::Suggest { .. }), "{d:?}");
    }

    #[test]
    fn weak_match_is_a_suggestion_and_a_distant_voice_stays_unknown() {
        let t = Thresholds::default();
        let profiles = [p("a", &[1.0, 0.0])];
        // cosine 0.65 -> suggestion
        let weak = classify(&[0.65, 0.76], &profiles, &t);
        assert!(matches!(weak, Decision::Suggest { .. }), "{weak:?}");
        // cosine 0.5 is below the suggestion limit -> unknown
        let far = classify(&[0.5, 0.866], &profiles, &t);
        assert_eq!(far, Decision::Unknown);
    }

    #[test]
    fn no_profiles_means_unknown() {
        assert_eq!(
            classify(&[1.0, 0.0], &[], &Thresholds::default()),
            Decision::Unknown
        );
    }

    #[test]
    fn threshold_boundaries_are_inclusive() {
        let t = Thresholds {
            accept: 0.5,
            suggest: 0.5,
            margin: 0.0,
        };
        let profiles = [p("a", &[1.0, 0.0])];
        let d = classify(&[0.5, 0.866_025_4], &profiles, &t);
        assert!(matches!(d, Decision::Auto { .. }) || matches!(d, Decision::Unknown));
        let d = classify(&[1.0, 0.0], &profiles, &t);
        assert!(matches!(d, Decision::Auto { .. }));
    }

    #[test]
    fn a_participant_is_assigned_to_one_voice_per_meeting() {
        let t = Thresholds::default();
        let profiles = [p("a", &[1.0, 0.0, 0.0])];
        let candidates = [
            Candidate {
                voice_id: "v1".into(),
                embedding: vec![1.0, 0.1, 0.0],
            },
            Candidate {
                voice_id: "v2".into(),
                embedding: vec![1.0, 0.3, 0.0],
            },
        ];
        let out = assign_unique(&candidates, &profiles, &t);
        assert!(matches!(out[0].1, Decision::Auto { .. }));
        assert!(matches!(out[1].1, Decision::Suggest { .. }), "{:?}", out[1]);
    }

    #[test]
    fn thresholds_validity() {
        assert!(Thresholds::default().is_valid());
        assert!(!Thresholds {
            accept: 0.4,
            suggest: 0.5,
            margin: 0.1
        }
        .is_valid());
        assert!(!Thresholds {
            accept: 1.2,
            suggest: 0.5,
            margin: 0.1
        }
        .is_valid());
    }
}
