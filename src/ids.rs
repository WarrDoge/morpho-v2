//! Ids drawn like CPython's `random.Random(seed).getrandbits(128)` so replays reproduce them.

use rand_mt::Mt;
use sha2::{Digest, Sha512};

#[derive(Clone)]
pub struct IdGen(Mt);

impl IdGen {
    pub fn random() -> Self {
        let mut buf = [0u8; 16];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = std::io::Read::read_exact(&mut f, &mut buf);
        }
        let key: Vec<u32> = buf
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        IdGen(Mt::new_with_key(key.iter().copied()))
    }

    /// CPython `random.seed(str)`: the big-endian integer of `utf8 ++ sha512(utf8)` as LE words.
    pub fn seeded(seed: &str) -> Self {
        let mut bytes = seed.as_bytes().to_vec();
        bytes.extend_from_slice(&Sha512::digest(seed.as_bytes()));
        let start = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
        let bytes = &bytes[start..];
        let mut key = vec![0u32; bytes.len().div_ceil(4).max(1)];
        for (i, b) in bytes.iter().rev().enumerate() {
            key[i / 4] |= (*b as u32) << (8 * (i % 4));
        }
        while key.len() > 1 && *key.last().unwrap() == 0 {
            key.pop();
        }
        IdGen(Mt::new_with_key(key.iter().copied()))
    }

    pub fn next(&mut self, prefix: &str) -> String {
        let w: Vec<u32> = (0..4).map(|_| self.0.next_u32()).collect();
        format!("{prefix}_{:08x}{:04x}", w[3], w[2] >> 16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_vector() {
        let mut g = IdGen::seeded("dana");
        let ids: Vec<String> = (0..5).map(|_| g.next("prop")).collect();
        assert_eq!(
            ids,
            [
                "prop_9eb885a1bb3d",
                "prop_fd34b497ce66",
                "prop_5823b43cb0d5",
                "prop_3a5d38b178df",
                "prop_23ba38e3a25e"
            ]
        );
    }
}
