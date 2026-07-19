// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
//! Small std-only SHA-256 seam for content and chain digests.
//!
//! Contract: bytes are hashed incrementally with the SHA-256 compression function;
//! length overflow is returned instead of wrapping. This module does not implement
//! signatures, keys, authentication, or algorithm negotiation.

use std::io;

const INITIAL_STATE: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

const ROUND_CONSTANTS: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

#[derive(Clone)]
pub(crate) struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    block_len: usize,
    total_bytes: u64,
}

impl Sha256 {
    pub(crate) fn new() -> Self {
        Self {
            state: INITIAL_STATE,
            block: [0; 64],
            block_len: 0,
            total_bytes: 0,
        }
    }

    pub(crate) fn update(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        self.total_bytes = self
            .total_bytes
            .checked_add(u64::try_from(bytes.len()).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("SHA-256 input length overflow"))?;
        if self.block_len != 0 {
            let needed = 64 - self.block_len;
            let copied = needed.min(bytes.len());
            self.block[self.block_len..self.block_len + copied].copy_from_slice(&bytes[..copied]);
            self.block_len += copied;
            bytes = &bytes[copied..];
            if self.block_len == 64 {
                let block = self.block;
                self.compress(&block);
                self.block_len = 0;
            } else {
                return Ok(());
            }
        }
        while bytes.len() >= 64 {
            let (block, remaining) = bytes.split_at(64);
            let block: &[u8; 64] = block
                .try_into()
                .map_err(|_| io::Error::other("SHA-256 block conversion failed"))?;
            self.compress(block);
            bytes = remaining;
        }
        self.block[..bytes.len()].copy_from_slice(bytes);
        self.block_len = bytes.len();
        Ok(())
    }

    pub(crate) fn digest(&self) -> io::Result<[u8; 32]> {
        let mut finalizer = self.clone();
        let bit_length = finalizer
            .total_bytes
            .checked_mul(8)
            .ok_or_else(|| io::Error::other("SHA-256 bit length overflow"))?;
        finalizer.block[finalizer.block_len] = 0x80;
        finalizer.block_len += 1;
        if finalizer.block_len > 56 {
            finalizer.block[finalizer.block_len..].fill(0);
            let block = finalizer.block;
            finalizer.compress(&block);
            finalizer.block = [0; 64];
            finalizer.block_len = 0;
        }
        finalizer.block[finalizer.block_len..56].fill(0);
        finalizer.block[56..64].copy_from_slice(&bit_length.to_be_bytes());
        let block = finalizer.block;
        finalizer.compress(&block);

        let mut digest = [0_u8; 32];
        for (word, output) in finalizer.state.iter().zip(digest.chunks_exact_mut(4)) {
            output.copy_from_slice(&word.to_be_bytes());
        }
        Ok(digest)
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut schedule = [0_u32; 64];
        for (index, bytes) in block.chunks_exact(4).enumerate() {
            schedule[index] = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for index in 16..64 {
            let small_0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let small_1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(small_0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(small_1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for (constant, scheduled) in ROUND_CONSTANTS.iter().zip(schedule) {
            let choose = (e & f) ^ (!e & g);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let big_0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let big_1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let temporary_1 = h
                .wrapping_add(big_1)
                .wrapping_add(choose)
                .wrapping_add(*constant)
                .wrapping_add(scheduled);
            let temporary_2 = big_0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary_1);
            d = c;
            c = b;
            b = a;
            a = temporary_1.wrapping_add(temporary_2);
        }
        for (state, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *state = state.wrapping_add(value);
        }
    }
}

pub(crate) fn digest_parts(parts: &[&[u8]]) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part)?;
    }
    hasher.digest()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{digest_parts, hex, Sha256};

    #[test]
    fn sha256_matches_published_empty_and_abc_vectors() {
        assert_eq!(
            hex(&digest_parts(&[b""]).expect("hash empty vector")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&digest_parts(&[b"abc"]).expect("hash abc vector")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn incremental_sha256_matches_one_shot_across_block_boundaries() {
        let input = (0_u8..=255).cycle().take(1_001).collect::<Vec<_>>();
        let expected = digest_parts(&[&input]).expect("one-shot digest");
        let mut incremental = Sha256::new();
        for chunk in input.chunks(7) {
            incremental
                .update(chunk)
                .expect("incremental digest update");
        }
        assert_eq!(incremental.digest().expect("incremental digest"), expected);
    }
}
