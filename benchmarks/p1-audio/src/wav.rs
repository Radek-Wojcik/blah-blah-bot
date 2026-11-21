//! Minimal RIFF/WAVE support: 16-bit mono PCM header and a reader for analysis.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Size of the canonical header written by [`header`].
pub const HEADER_LEN: usize = 44;

/// Build a canonical 44-byte header for 16-bit PCM.
pub fn header(sample_rate: u32, channels: u16, data_len: u32) -> [u8; HEADER_LEN] {
    let bits: u16 = 16;
    let block_align = channels * bits / 8;
    let byte_rate = sample_rate * u32::from(block_align);
    let mut h = [0u8; HEADER_LEN];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    h[32..34].copy_from_slice(&block_align.to_le_bytes());
    h[34..36].copy_from_slice(&bits.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

/// Fields of a parsed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub data_len: u32,
}

/// Parse a canonical 44-byte header.
pub fn parse_header(h: &[u8]) -> Result<Info, String> {
    if h.len() < HEADER_LEN {
        return Err("header too short".into());
    }
    if &h[0..4] != b"RIFF" || &h[8..12] != b"WAVE" || &h[12..16] != b"fmt " {
        return Err("not a canonical WAVE file".into());
    }
    if &h[36..40] != b"data" {
        return Err("data chunk is not at offset 36".into());
    }
    let u16_at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
    Ok(Info {
        sample_rate: u32_at(24),
        channels: u16_at(22),
        bits: u16_at(34),
        data_len: u32_at(40),
    })
}

/// Writer for one segment file. The data length in the header is patched on [`finish`].
///
/// [`finish`]: SegmentFile::finish
pub struct SegmentFile {
    file: io::BufWriter<File>,
    sample_rate: u32,
    samples: u64,
}

impl SegmentFile {
    pub fn create(path: &Path, sample_rate: u32) -> io::Result<Self> {
        let mut file = io::BufWriter::new(File::create(path)?);
        file.write_all(&header(sample_rate, 1, 0))?;
        Ok(Self {
            file,
            sample_rate,
            samples: 0,
        })
    }

    pub fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.file.write_all(&bytes)?;
        self.samples += samples.len() as u64;
        Ok(())
    }

    /// Push buffered bytes to the operating system.
    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    /// Patch the header with the final length and close the file.
    pub fn finish(mut self) -> io::Result<u64> {
        self.file.flush()?;
        let data_len = (self.samples * 2) as u32;
        let inner = self.file.get_mut();
        inner.seek(SeekFrom::Start(0))?;
        inner.write_all(&header(self.sample_rate, 1, data_len))?;
        inner.flush()?;
        Ok(self.samples)
    }
}

/// Read a whole mono 16-bit file. When the header length field is 0 or too large (a file left
/// by a crash) the length of the file decides.
pub fn read_samples(path: &Path) -> Result<(Info, Vec<i16>), String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut all = Vec::new();
    f.read_to_end(&mut all).map_err(|e| e.to_string())?;
    let info = parse_header(&all)?;
    let body = &all[HEADER_LEN..];
    let declared = info.data_len as usize;
    let len = if declared == 0 || declared > body.len() {
        body.len()
    } else {
        declared
    };
    let samples = body[..len - len % 2]
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok((info, samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> crate::test_util::TempDir {
        crate::test_util::TempDir::new(name)
    }

    #[test]
    fn header_round_trip() {
        let h = header(16_000, 1, 32_000);
        let info = parse_header(&h).unwrap();
        assert_eq!(
            info,
            Info {
                sample_rate: 16_000,
                channels: 1,
                bits: 16,
                data_len: 32_000
            }
        );
        assert_eq!(u32::from_le_bytes([h[4], h[5], h[6], h[7]]), 36 + 32_000);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_header(&[0u8; 10]).is_err());
        assert!(parse_header(&[0u8; 44]).is_err());
    }

    #[test]
    fn segment_file_round_trip() {
        let dir = temp_dir("wav");
        let path = dir.path().join("a.wav");
        let mut f = SegmentFile::create(&path, 16_000).unwrap();
        f.write(&[1, -2, 3]).unwrap();
        f.write(&[i16::MAX, i16::MIN]).unwrap();
        assert_eq!(f.finish().unwrap(), 5);
        let (info, samples) = read_samples(&path).unwrap();
        assert_eq!(info.data_len, 10);
        assert_eq!(samples, vec![1, -2, 3, i16::MAX, i16::MIN]);
    }

    #[test]
    fn unfinished_file_is_readable() {
        let dir = temp_dir("wav-crash");
        let path = dir.path().join("crash.wav");
        let mut f = SegmentFile::create(&path, 16_000).unwrap();
        f.write(&[5, 6, 7, 8]).unwrap();
        f.flush().unwrap();
        // The header still says 0 bytes, as after a crash.
        let (_, samples) = read_samples(&path).unwrap();
        assert_eq!(samples, vec![5, 6, 7, 8]);
        drop(f);
    }
}
