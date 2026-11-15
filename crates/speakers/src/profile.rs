//! Voice profile: a set of samples (one per confirmed voice) and the centroid computed
//! from them.

use crate::matching::normalize;

/// Most samples kept per participant. When full, the shortest sample is replaced by a longer
/// one, so the profile keeps the best evidence.
pub const MAX_SAMPLES: usize = 20;

/// Voices with less speech than this are not added to a profile: their embedding is too noisy.
pub const MIN_SAMPLE_MS: u64 = 5_000;

/// One confirmed voice from one meeting.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// Identifies the source (meeting and voice), so a re-confirmation replaces the old sample.
    pub key: String,
    pub embedding: Vec<f32>,
    pub speech_ms: u64,
}

/// What happened when a sample was offered to a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    Added,
    Replaced,
    /// Too little speech to learn from.
    TooShort,
    /// The profile is full of longer samples.
    NotBetter,
}

/// Samples of one participant.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SampleSet {
    samples: Vec<Sample>,
}

impl SampleSet {
    pub fn new(samples: Vec<Sample>) -> Self {
        Self { samples }
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Offer a confirmed voice. The same key replaces its earlier sample (the user changed
    /// their mind or confirmed again); a new key is added, evicting the shortest sample when
    /// the profile is full and the newcomer is longer.
    pub fn add(&mut self, sample: Sample) -> AddOutcome {
        if sample.speech_ms < MIN_SAMPLE_MS {
            return AddOutcome::TooShort;
        }
        if let Some(existing) = self.samples.iter_mut().find(|s| s.key == sample.key) {
            *existing = sample;
            return AddOutcome::Replaced;
        }
        if self.samples.len() < MAX_SAMPLES {
            self.samples.push(sample);
            return AddOutcome::Added;
        }
        let (idx, shortest) = self
            .samples
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| s.speech_ms)
            .map(|(i, s)| (i, s.speech_ms))
            .expect("the set is full, so it is not empty");
        if sample.speech_ms > shortest {
            self.samples[idx] = sample;
            AddOutcome::Replaced
        } else {
            AddOutcome::NotBetter
        }
    }

    /// Remove the sample that came from one source. Returns whether one was removed.
    pub fn remove(&mut self, key: &str) -> bool {
        let before = self.samples.len();
        self.samples.retain(|s| s.key != key);
        self.samples.len() != before
    }

    /// The profile vector, or `None` without samples.
    pub fn centroid(&self) -> Option<Vec<f32>> {
        centroid(&self.samples)
    }
}

/// Unit-length mean of the normalized sample embeddings, weighted by speech time with the
/// weight capped at 60 s, so that one long meeting does not drown the others.
pub fn centroid(samples: &[Sample]) -> Option<Vec<f32>> {
    let dim = samples.first()?.embedding.len();
    if dim == 0 || samples.iter().any(|s| s.embedding.len() != dim) {
        return None;
    }
    let mut sum = vec![0.0f64; dim];
    for s in samples {
        let weight = (s.speech_ms.min(60_000) as f64) / 1000.0;
        for (acc, x) in sum.iter_mut().zip(normalize(&s.embedding)) {
            *acc += f64::from(x) * weight;
        }
    }
    let as_f32: Vec<f32> = sum.iter().map(|x| *x as f32).collect();
    Some(normalize(&as_f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matching::cosine;

    fn sample(key: &str, v: &[f32], ms: u64) -> Sample {
        Sample {
            key: key.into(),
            embedding: v.to_vec(),
            speech_ms: ms,
        }
    }

    #[test]
    fn centroid_is_the_weighted_unit_mean() {
        let c = centroid(&[
            sample("a", &[1.0, 0.0], 10_000),
            sample("b", &[0.0, 1.0], 30_000),
        ])
        .unwrap();
        assert!((cosine(&c, &[1.0, 3.0]) - 1.0).abs() < 1e-5, "{c:?}");
        let len: f32 = c.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((len - 1.0).abs() < 1e-5);
    }

    #[test]
    fn weight_is_capped() {
        let c = centroid(&[
            sample("a", &[1.0, 0.0], 60_000),
            sample("b", &[0.0, 1.0], 600_000),
        ])
        .unwrap();
        assert!((c[0] - c[1]).abs() < 1e-5);
    }

    #[test]
    fn no_samples_or_mixed_sizes_give_no_centroid() {
        assert!(centroid(&[]).is_none());
        assert!(centroid(&[sample("a", &[1.0], 9_000), sample("b", &[1.0, 0.0], 9_000)]).is_none());
    }

    #[test]
    fn short_voices_are_not_learned() {
        let mut set = SampleSet::default();
        assert_eq!(
            set.add(sample("a", &[1.0, 0.0], 4_999)),
            AddOutcome::TooShort
        );
        assert!(set.is_empty());
        assert_eq!(set.add(sample("a", &[1.0, 0.0], 5_000)), AddOutcome::Added);
    }

    #[test]
    fn the_same_source_replaces_its_sample() {
        let mut set = SampleSet::default();
        set.add(sample("m1:v1", &[1.0, 0.0], 9_000));
        assert_eq!(
            set.add(sample("m1:v1", &[0.0, 1.0], 12_000)),
            AddOutcome::Replaced
        );
        assert_eq!(set.len(), 1);
        assert_eq!(set.samples()[0].embedding, vec![0.0, 1.0]);
    }

    #[test]
    fn a_full_profile_keeps_the_longest_samples() {
        let mut set = SampleSet::default();
        for i in 0..MAX_SAMPLES {
            set.add(sample(
                &format!("k{i}"),
                &[1.0, 0.0],
                6_000 + i as u64 * 100,
            ));
        }
        assert_eq!(
            set.add(sample("short", &[1.0, 0.0], 5_500)),
            AddOutcome::NotBetter
        );
        assert_eq!(
            set.add(sample("long", &[1.0, 0.0], 50_000)),
            AddOutcome::Replaced
        );
        assert_eq!(set.len(), MAX_SAMPLES);
        assert!(!set.samples().iter().any(|s| s.key == "k0"));
    }

    #[test]
    fn removing_a_sample_by_source() {
        let mut set = SampleSet::default();
        set.add(sample("a", &[1.0, 0.0], 9_000));
        assert!(set.remove("a"));
        assert!(!set.remove("a"));
        assert!(set.centroid().is_none());
    }
}
