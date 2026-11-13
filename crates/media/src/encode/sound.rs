//! Turning the mixer's interleaved float samples into the layout a sound
//! encoder takes. Pure, so it is tested without FFmpeg.

/// How an encoder wants its samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// Floats, one plane per channel.
    FloatPlanes,
    /// Floats, channels interleaved.
    Float,
    /// 32-bit whole numbers, one plane per channel.
    WholePlanes,
    /// 32-bit whole numbers, interleaved.
    Whole,
    /// 16-bit whole numbers, one plane per channel.
    ShortPlanes,
    /// 16-bit whole numbers, interleaved.
    Short,
}

impl Layout {
    /// The order a layout is chosen in, when the encoder takes several:
    /// floats lose nothing, then the widest whole numbers.
    pub(crate) const PREFERRED: [Layout; 6] = [
        Layout::FloatPlanes,
        Layout::Float,
        Layout::WholePlanes,
        Layout::Whole,
        Layout::ShortPlanes,
        Layout::Short,
    ];

    pub(crate) fn planar(self) -> bool {
        matches!(
            self,
            Layout::FloatPlanes | Layout::WholePlanes | Layout::ShortPlanes
        )
    }

    pub(crate) fn bytes_per_sample(self) -> usize {
        match self {
            Layout::ShortPlanes | Layout::Short => 2,
            _ => 4,
        }
    }
}

/// A float sample as a 32-bit whole number holding `bits` significant
/// bits (24 for 24-bit sound, which sits in the top bits), clipped.
pub(crate) fn whole(sample: f32, bits: u32) -> i32 {
    let top = ((1i64 << (bits - 1)) - 1) as f64;
    let value = (f64::from(sample).clamp(-1.0, 1.0) * top).round() as i64;
    (value << (32 - bits)) as i32
}

/// A float sample as a 16-bit whole number, clipped.
pub(crate) fn short(sample: f32) -> i16 {
    (f64::from(sample).clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Writes `samples` (interleaved, `channels` a frame) into `planes`: one
/// byte buffer per channel for a planar layout, else only the first.
/// `bits` is how many bits whole numbers carry (24 or 32).
pub(crate) fn write(samples: &[f32], channels: usize, layout: Layout, bits: u32, planes: &mut [&mut [u8]]) {
    let size = layout.bytes_per_sample();
    for (i, frame) in samples.chunks_exact(channels).enumerate() {
        for (channel, &sample) in frame.iter().enumerate() {
            let (plane, slot) = if layout.planar() {
                (channel, i)
            } else {
                (0, i * channels + channel)
            };
            let Some(out) = planes
                .get_mut(plane)
                .and_then(|p| p.get_mut(slot * size..(slot + 1) * size))
            else {
                continue;
            };
            match layout {
                Layout::FloatPlanes | Layout::Float => out.copy_from_slice(&sample.to_ne_bytes()),
                Layout::WholePlanes | Layout::Whole => {
                    out.copy_from_slice(&whole(sample, bits).to_ne_bytes())
                }
                Layout::ShortPlanes | Layout::Short => out.copy_from_slice(&short(sample).to_ne_bytes()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_numbers_keep_24_bits_in_the_top_and_clip() {
        assert_eq!(whole(0.0, 24), 0);
        assert_eq!(whole(1.0, 24), 8_388_607 << 8);
        assert_eq!(whole(-1.0, 24), -8_388_607 << 8);
        assert_eq!(whole(2.0, 24), whole(1.0, 24), "clipped");
        assert_eq!(whole(0.5, 24) >> 8, 4_194_304);
        assert_eq!(whole(1.0, 32), i32::MAX);
        assert_eq!(short(-3.0), -32767);
        assert_eq!(short(0.5), 16384);
    }

    #[test]
    fn samples_land_in_planes_or_interleaved() {
        let samples = [0.25f32, -0.5, 0.75, 1.0];
        let mut left = vec![0u8; 8];
        let mut right = vec![0u8; 8];
        write(&samples, 2, Layout::FloatPlanes, 32, &mut [&mut left, &mut right]);
        let floats = |bytes: &[u8]| -> Vec<f32> {
            bytes
                .chunks_exact(4)
                .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
                .collect()
        };
        assert_eq!(floats(&left), vec![0.25, 0.75]);
        assert_eq!(floats(&right), vec![-0.5, 1.0]);

        let mut one = vec![0u8; 8];
        write(&samples, 2, Layout::Short, 32, &mut [&mut one]);
        let shorts: Vec<i16> = one
            .chunks_exact(2)
            .map(|b| i16::from_ne_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(shorts, vec![8192, -16384, 24575, 32767]);

        let mut whole_planes = [vec![0u8; 8], vec![0u8; 8]];
        let [a, b] = &mut whole_planes;
        write(&samples, 2, Layout::WholePlanes, 24, &mut [a, b]);
        let first = i32::from_ne_bytes([a[0], a[1], a[2], a[3]]);
        assert_eq!(first, whole(0.25, 24));
    }
}
