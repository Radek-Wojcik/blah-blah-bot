//! Conversion of captured frames to 16 kHz mono 16-bit samples.

/// Format requested from WASAPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 16 kHz mono 16-bit integer, converted by the audio engine.
    Pcm16Mono16k,
    /// 48 kHz stereo 32-bit float, converted by this program.
    Float32Stereo48k,
}

impl Format {
    /// Human-readable description.
    pub fn label(self) -> &'static str {
        match self {
            Format::Pcm16Mono16k => "16 kHz mono i16 (engine conversion)",
            Format::Float32Stereo48k => "48 kHz stereo f32 (own resampling)",
        }
    }

    /// Bytes per frame as delivered by the device.
    pub fn frame_bytes(self) -> usize {
        match self {
            Format::Pcm16Mono16k => 2,
            Format::Float32Stereo48k => 8,
        }
    }
}

/// Stateful converter (keeps a partial group of frames between calls).
#[derive(Debug, Clone)]
pub struct Converter {
    format: Format,
    acc: f32,
    count: u32,
}

fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

impl Converter {
    /// Create a converter for frames delivered in `format`.
    pub fn new(format: Format) -> Self {
        Self {
            format,
            acc: 0.0,
            count: 0,
        }
    }

    /// Convert whole frames from `bytes` and append the result to `out`.
    ///
    /// The fallback format is reduced by averaging groups of three mono frames, a crude
    /// low-pass filter. It is only a fallback for devices that refuse the 16 kHz format; the
    /// audio engine's own conversion is used whenever it is accepted.
    pub fn convert(&mut self, bytes: &[u8], out: &mut Vec<i16>) {
        match self.format {
            Format::Pcm16Mono16k => {
                out.extend(
                    bytes
                        .chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]])),
                );
            }
            Format::Float32Stereo48k => {
                for frame in bytes.chunks_exact(8) {
                    let l = f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
                    let r = f32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
                    self.acc += (l + r) * 0.5;
                    self.count += 1;
                    if self.count == 3 {
                        out.push(to_i16(self.acc / 3.0));
                        self.acc = 0.0;
                        self.count = 0;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_frames(frames: &[(f32, f32)]) -> Vec<u8> {
        frames
            .iter()
            .flat_map(|(l, r)| l.to_le_bytes().into_iter().chain(r.to_le_bytes()))
            .collect()
    }

    #[test]
    fn native_format_is_copied() {
        let mut c = Converter::new(Format::Pcm16Mono16k);
        let mut out = Vec::new();
        c.convert(&[1, 0, 0xFF, 0xFF], &mut out);
        assert_eq!(out, vec![1, -1]);
    }

    #[test]
    fn fallback_mixes_and_decimates_across_calls() {
        let mut c = Converter::new(Format::Float32Stereo48k);
        let mut out = Vec::new();
        c.convert(&f32_frames(&[(1.0, 1.0), (1.0, 1.0)]), &mut out);
        assert!(out.is_empty());
        c.convert(&f32_frames(&[(1.0, 1.0), (-1.0, -1.0)]), &mut out);
        assert_eq!(out, vec![32767]);
        c.convert(&f32_frames(&[(-1.0, -1.0), (-1.0, -1.0)]), &mut out);
        assert_eq!(out, vec![32767, -32767]);
    }

    #[test]
    fn fallback_clamps() {
        assert_eq!(to_i16(2.0), 32767);
        assert_eq!(to_i16(-2.0), -32767);
    }

    #[test]
    fn frame_sizes() {
        assert_eq!(Format::Pcm16Mono16k.frame_bytes(), 2);
        assert_eq!(Format::Float32Stereo48k.frame_bytes(), 8);
    }
}
