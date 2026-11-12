//! Where the samples go. The chip does not know and does not care.

use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

pub trait Sink {
    type Error;
    /// One block of mono samples at the chip's host sample rate.
    fn write(&mut self, samples: &[f32]) -> Result<(), Self::Error>;
    /// Flush and close. Called once on clean shutdown.
    fn finish(&mut self) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    Pcm16,
    F32,
}

#[derive(Debug, Clone, Copy)]
pub struct WavSpec {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
}

impl WavSpec {
    fn bytes_per_frame(self) -> u64 {
        u64::from(self.channels)
            * match self.format {
                SampleFormat::Pcm16 => 2,
                SampleFormat::F32 => 4,
            }
    }
}

pub struct WavSink {
    file: std::fs::File,
    spec: WavSpec,
    frames: u64,
}

impl WavSink {
    pub fn create(path: &Path, spec: WavSpec) -> io::Result<Self> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(&Self::header(spec, 0, 0))?;
        Ok(Self {
            file,
            spec,
            frames: 0,
        })
    }

    fn header(spec: WavSpec, riff_len: u32, data_len: u32) -> [u8; 44] {
        let (tag, bits) = match spec.format {
            SampleFormat::Pcm16 => (1u16, 16u16),
            SampleFormat::F32 => (3, 32),
        };
        let block_align = spec.channels * bits / 8;
        let byte_rate = spec.sample_rate * u32::from(block_align);
        let mut h = [0u8; 44];
        h[0..4].copy_from_slice(b"RIFF");
        h[4..8].copy_from_slice(&riff_len.to_le_bytes());
        h[8..12].copy_from_slice(b"WAVE");
        h[12..16].copy_from_slice(b"fmt ");
        h[16..20].copy_from_slice(&16u32.to_le_bytes());
        h[20..22].copy_from_slice(&tag.to_le_bytes());
        h[22..24].copy_from_slice(&spec.channels.to_le_bytes());
        h[24..28].copy_from_slice(&spec.sample_rate.to_le_bytes());
        h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        h[32..34].copy_from_slice(&block_align.to_le_bytes());
        h[34..36].copy_from_slice(&bits.to_le_bytes());
        h[36..40].copy_from_slice(b"data");
        h[40..44].copy_from_slice(&data_len.to_le_bytes());
        h
    }

    pub fn samples_written(&self) -> u64 {
        self.frames
    }
}

impl Sink for WavSink {
    type Error = io::Error;

    fn write(&mut self, samples: &[f32]) -> Result<(), io::Error> {
        let mut buf = Vec::with_capacity(samples.len() * usize::from(self.spec.channels) * 4);
        for &s in samples {
            let s = s.clamp(-1.0, 1.0);
            for _ in 0..self.spec.channels {
                match self.spec.format {
                    SampleFormat::Pcm16 => {
                        // Full negative scale is one step wider than full positive
                        // scale; the float-to-int cast saturates, so +1.0 lands on
                        // 32767 and -1.0 on -32768 without special-casing either end.
                        let v = (s * 32_768.0).round() as i16;
                        buf.extend_from_slice(&v.to_le_bytes());
                    }
                    SampleFormat::F32 => buf.extend_from_slice(&s.to_le_bytes()),
                }
            }
        }
        self.file.write_all(&buf)?;
        self.frames += samples.len() as u64;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), io::Error> {
        let data_len = self.frames * self.spec.bytes_per_frame();
        let file_len = 44 + data_len;
        if data_len > u64::from(u32::MAX) || file_len > u64::from(u32::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "WAV longer than 4 GiB; split the render",
            ));
        }
        let header = Self::header(self.spec, (file_len - 8) as u32, data_len as u32);
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header)?;
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn temp(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "rust-sid-{}-{}-{}.wav",
            name,
            std::process::id(),
            // Two tests in the same process must still not share a file.
            {
                use std::sync::atomic::{AtomicU64, Ordering};
                static SEQ: AtomicU64 = AtomicU64::new(0);
                SEQ.fetch_add(1, Ordering::Relaxed)
            }
        ));
        p
    }

    fn header(path: &std::path::Path) -> Vec<u8> {
        let mut f = std::fs::File::open(path).unwrap();
        let mut b = vec![0u8; 44];
        f.read_exact(&mut b).unwrap();
        b
    }

    #[test]
    fn writes_a_canonical_riff_header() {
        let path = temp("hdr");
        let spec = WavSpec {
            sample_rate: 44_100,
            channels: 1,
            format: SampleFormat::Pcm16,
        };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[0.0; 128]).unwrap();
        sink.finish().unwrap();

        let h = header(&path);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(&h[8..12], b"WAVE");
        assert_eq!(&h[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(h[16..20].try_into().unwrap()), 16);
        assert_eq!(
            u16::from_le_bytes(h[20..22].try_into().unwrap()),
            1,
            "PCM tag"
        );
        assert_eq!(u16::from_le_bytes(h[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 44_100);
        assert_eq!(
            u16::from_le_bytes(h[34..36].try_into().unwrap()),
            16,
            "bits"
        );
        assert_eq!(&h[36..40], b"data");
        assert_eq!(
            u32::from_le_bytes(h[40..44].try_into().unwrap()),
            256,
            "byte count"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn patches_both_length_fields_on_finish() {
        let path = temp("patch");
        let spec = WavSpec {
            sample_rate: 22_050,
            channels: 2,
            format: SampleFormat::Pcm16,
        };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[0.25; 100]).unwrap();
        sink.finish().unwrap();

        let file_len = std::fs::metadata(&path).unwrap().len();
        let h = header(&path);
        let riff = u32::from_le_bytes(h[4..8].try_into().unwrap()) as u64;
        let data = u32::from_le_bytes(h[40..44].try_into().unwrap()) as u64;
        assert_eq!(riff, file_len - 8, "RIFF size not patched");
        // 100 mono frames duplicated onto two channels, 2 bytes each.
        assert_eq!(data, 400, "data size not patched");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn float_format_is_tagged_as_ieee() {
        let path = temp("f32");
        let spec = WavSpec {
            sample_rate: 44_100,
            channels: 1,
            format: SampleFormat::F32,
        };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[0.0; 4]).unwrap();
        sink.finish().unwrap();
        let h = header(&path);
        assert_eq!(
            u16::from_le_bytes(h[20..22].try_into().unwrap()),
            3,
            "IEEE float tag"
        );
        assert_eq!(u16::from_le_bytes(h[34..36].try_into().unwrap()), 32);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn samples_are_clamped_to_full_scale() {
        let path = temp("clamp");
        let spec = WavSpec {
            sample_rate: 44_100,
            channels: 1,
            format: SampleFormat::Pcm16,
        };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[9.0, -9.0]).unwrap();
        sink.finish().unwrap();

        let body = std::fs::read(&path).unwrap();
        let samples: Vec<i16> = body[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(samples, vec![32_767, -32_768]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_sink_can_be_swapped_without_changing_the_caller() {
        // The point of the trait: this function knows nothing about WAV.
        fn pump<S: Sink<Error = std::io::Error>>(sink: &mut S) -> std::io::Result<()> {
            for _ in 0..4 {
                sink.write(&[0.1, 0.2, 0.3, 0.4])?;
            }
            sink.finish()
        }
        let path = temp("swap");
        let mut sink = WavSink::create(
            &path,
            WavSpec {
                sample_rate: 8_000,
                channels: 1,
                format: SampleFormat::Pcm16,
            },
        )
        .unwrap();
        pump(&mut sink).unwrap();
        assert_eq!(sink.samples_written(), 16);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_sink_works_behind_a_trait_object() {
        // The playback loop in a later task holds `Box<dyn Sink>`; if the trait ever
        // stops being object safe that loop stops compiling, and this fails first.
        let path = temp("dyn");
        let sink = WavSink::create(
            &path,
            WavSpec {
                sample_rate: 44_100,
                channels: 1,
                format: SampleFormat::Pcm16,
            },
        )
        .unwrap();
        let mut boxed: Box<dyn Sink<Error = std::io::Error>> = Box::new(sink);
        boxed.write(&[0.5, -0.5]).unwrap();
        boxed.finish().unwrap();
        drop(boxed);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 44 + 4);
        std::fs::remove_file(&path).ok();
    }
}
