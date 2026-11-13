//! The fixed-block adapter: the sound card asks for whatever buffer size
//! it likes (128, 441, 480, 1024 frames...), the mixer always makes
//! [`BLOCK`] frames. The adapter keeps the rest of the last block for the
//! next call, so the sound is the same to the sample however the device
//! cuts it.

use crate::BLOCK;
use crate::mixer::Mixer;

#[derive(Debug)]
pub struct BlockAdapter {
    block: [f32; 2 * BLOCK],
    /// Frames of `block` already given out.
    used: usize,
}

impl Default for BlockAdapter {
    fn default() -> BlockAdapter {
        BlockAdapter {
            block: [0.0; 2 * BLOCK],
            used: BLOCK,
        }
    }
}

impl BlockAdapter {
    /// Fills `out` (interleaved, `channels` per frame) from the mixer:
    /// left and right in the first two channels and silence in the rest,
    /// or both mixed into one for a single channel.
    pub fn render(&mut self, mixer: &mut Mixer, out: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        for frame in out.chunks_exact_mut(channels) {
            if self.used == BLOCK {
                mixer.process(&mut self.block);
                self.used = 0;
            }
            let (l, r) = (self.block[2 * self.used], self.block[2 * self.used + 1]);
            self.used += 1;
            match frame {
                [one] => *one = 0.5 * (l + r),
                [a, b, rest @ ..] => {
                    *a = l;
                    *b = r;
                    rest.fill(0.0);
                }
                [] => {}
            }
        }
    }

    /// Frames of the last block made and not yet given out: the mixer's
    /// position is this far ahead of what the device was handed.
    pub fn pending(&self) -> usize {
        BLOCK - self.used
    }

    /// Forgets the rest of the last block (after a jump, so the next sound
    /// starts at the new place).
    pub fn clear(&mut self) {
        self.used = BLOCK;
    }
}
