//! Per-track segment writer: splits samples into segment files and journals each segment.

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::journal::{Entry, Journal};
use crate::timeline::{segment_offset_ms, Segmenter};
use crate::wav::SegmentFile;

/// File name of a segment.
pub fn segment_file_name(segment: u32) -> String {
    format!("seg-{segment:05}.wav")
}

/// Writes one track as consecutive segment files.
pub struct TrackWriter {
    name: String,
    dir: PathBuf,
    rate: u32,
    seg_samples: u64,
    offset_ms: f64,
    segmenter: Segmenter,
    current: Option<(u32, SegmentFile)>,
    journal: Arc<Mutex<Journal>>,
    since_flush: u64,
}

impl TrackWriter {
    /// Create the track directory and an empty writer.
    pub fn new(
        name: &str,
        root: &std::path::Path,
        journal: Arc<Mutex<Journal>>,
        offset_ms: f64,
        rate: u32,
        seg_samples: u64,
    ) -> io::Result<Self> {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            name: name.to_string(),
            dir,
            rate,
            seg_samples,
            offset_ms,
            segmenter: Segmenter::new(seg_samples),
            current: None,
            journal,
            since_flush: 0,
        })
    }

    /// Samples stored so far, including inserted silence.
    pub fn written(&self) -> u64 {
        self.segmenter.total()
    }

    fn log(&self, entry: Entry) -> io::Result<()> {
        self.journal
            .lock()
            .map_err(|_| io::Error::other("journal lock poisoned"))?
            .append(&entry)
    }

    fn close_current(&mut self) -> io::Result<()> {
        if let Some((segment, file)) = self.current.take() {
            let samples = file.finish()?;
            self.log(Entry::Segment {
                track: self.name.clone(),
                segment,
                file: format!("{}/{}", self.name, segment_file_name(segment)),
                offset_ms: segment_offset_ms(self.offset_ms, segment, self.seg_samples, self.rate),
                samples,
            })?;
        }
        Ok(())
    }

    /// Append samples.
    pub fn push(&mut self, samples: &[i16]) -> io::Result<()> {
        for chunk in self.segmenter.push(samples.len()) {
            if self
                .current
                .as_ref()
                .is_some_and(|(s, _)| *s != chunk.segment)
            {
                self.close_current()?;
            }
            if self.current.is_none() {
                let path = self.dir.join(segment_file_name(chunk.segment));
                self.current = Some((chunk.segment, SegmentFile::create(&path, self.rate)?));
            }
            if let Some((_, file)) = self.current.as_mut() {
                file.write(&samples[chunk.start..chunk.start + chunk.len])?;
            }
            if chunk.completes {
                self.close_current()?;
                self.since_flush = 0;
            } else {
                // Push data to the operating system about once per second of audio, so
                // a crash loses at most that much from the open segment file.
                self.since_flush += chunk.len as u64;
                if self.since_flush >= u64::from(self.rate) {
                    self.since_flush = 0;
                    if let Some((_, file)) = self.current.as_mut() {
                        file.flush()?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Append `count` samples of silence.
    pub fn push_silence(&mut self, count: u64) -> io::Result<()> {
        let zeros = vec![0i16; 16_000];
        let mut left = count;
        while left > 0 {
            let n = left.min(zeros.len() as u64) as usize;
            self.push(&zeros[..n])?;
            left -= n as u64;
        }
        Ok(())
    }

    /// Close the last (possibly partial) segment and return the total sample count.
    pub fn finish(mut self) -> io::Result<u64> {
        self.close_current()?;
        Ok(self.segmenter.total())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::replay_file;
    use crate::wav::read_samples;

    #[test]
    fn writes_segments_and_journal() {
        let root = std::env::temp_dir().join(format!("p1-audio-track-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let journal = Arc::new(Mutex::new(
            Journal::create(&root.join("journal.jsonl")).unwrap(),
        ));
        journal
            .lock()
            .unwrap()
            .append(&Entry::TrackStart {
                track: "t".into(),
                source: "mic".into(),
                pid: None,
                include_tree: None,
                offset_ms: 5.0,
            })
            .unwrap();
        let mut w = TrackWriter::new("t", &root, journal, 5.0, 16_000, 100).unwrap();
        let data: Vec<i16> = (0..250).map(|i| i as i16).collect();
        w.push(&data[..70]).unwrap();
        w.push(&data[70..]).unwrap();
        w.push_silence(10).unwrap();
        assert_eq!(w.written(), 260);
        assert_eq!(w.finish().unwrap(), 260);

        let (_, s0) = read_samples(&root.join("t/seg-00000.wav")).unwrap();
        let (_, s1) = read_samples(&root.join("t/seg-00001.wav")).unwrap();
        let (_, s2) = read_samples(&root.join("t/seg-00002.wav")).unwrap();
        assert_eq!(s0, data[..100]);
        assert_eq!(s1, data[100..200]);
        assert_eq!(s2.len(), 60);
        assert_eq!(s2[..50], data[200..]);
        assert!(s2[50..].iter().all(|&x| x == 0));

        let r = replay_file(&root.join("journal.jsonl")).unwrap();
        let t = &r.tracks["t"];
        assert_eq!(t.segments.len(), 3);
        assert_eq!(t.segments[2].samples, 60);
        assert_eq!(t.segments[1].offset_ms, 5.0 + 100.0 * 1000.0 / 16_000.0);
        assert!(
            t.problems(100, 16_000).is_empty(),
            "{:?}",
            t.problems(100, 16_000)
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn segment_names_are_zero_padded() {
        assert_eq!(segment_file_name(0), "seg-00000.wav");
        assert_eq!(segment_file_name(119), "seg-00119.wav");
    }
}
