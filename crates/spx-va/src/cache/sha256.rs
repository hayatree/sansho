//! SHA-256 for checksumming large private artifacts.
//! Provenance: FIPS 180-4 section 6.2 and Intel SHA Extensions instruction semantics.
#![allow(unsafe_code)]

use std::arch::x86_64::*;

// The caller dispatches only on CPUs supporting all three instruction sets.
#[target_feature(enable = "sha,ssse3,sse4.1")]
fn hash(bytes: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    // SHA256RNDS2 uses A,B,E,F and C,D,G,H from most to least significant lane.
    let mut abef = _mm_set_epi32(
        0x6a09e667,
        0xbb67ae85u32 as i32,
        0x510e527f,
        0x9b05688cu32 as i32,
    );
    let mut cdgh = _mm_set_epi32(0x3c6ef372, 0xa54ff53au32 as i32, 0x1f83d9ab, 0x5be0cd19);
    let (blocks, remainder) = bytes.as_chunks::<64>();
    let mut pad = [0u8; 128];
    let length = if remainder.len() < 56 { 64 } else { 128 };
    pad[..remainder.len()].copy_from_slice(remainder);
    pad[remainder.len()] = 0x80;
    pad[length - 8..length].copy_from_slice(&(bytes.len() as u64).wrapping_mul(8).to_be_bytes());
    let swap = _mm_setr_epi8(3, 2, 1, 0, 7, 6, 5, 4, 11, 10, 9, 8, 15, 14, 13, 12);
    for block in blocks.iter().chain(pad[..length].as_chunks::<64>().0) {
        let start = (abef, cdgh);
        let mut w = [_mm_setzero_si128(); 4];
        for (i, k) in K.as_chunks::<4>().0.iter().enumerate() {
            let j = i % 4;
            if i < 4 {
                // SAFETY: four 16-byte unaligned loads cover exactly this 64-byte block.
                w[j] = _mm_shuffle_epi8(
                    unsafe { _mm_loadu_si128(block.as_ptr().add(16 * j).cast()) },
                    swap,
                );
            } else {
                let next = _mm_sha256msg1_epu32(w[j], w[(j + 1) % 4]);
                let middle = _mm_alignr_epi8::<4>(w[(j + 3) % 4], w[(j + 2) % 4]);
                w[j] = _mm_sha256msg2_epu32(_mm_add_epi32(next, middle), w[(j + 3) % 4]);
            }
            // SAFETY: k contains exactly four 32-bit round constants.
            let wk = _mm_add_epi32(w[j], unsafe { _mm_loadu_si128(k.as_ptr().cast()) });
            cdgh = _mm_sha256rnds2_epu32(cdgh, abef, wk);
            abef = _mm_sha256rnds2_epu32(abef, cdgh, _mm_shuffle_epi32::<0xee>(wk));
        }
        abef = _mm_add_epi32(abef, start.0);
        cdgh = _mm_add_epi32(cdgh, start.1);
    }
    let mut result = [0u8; 32];
    let abcd = _mm_unpackhi_epi64(cdgh, abef);
    let efgh = _mm_unpacklo_epi64(cdgh, abef);
    // SAFETY: two 16-byte stores cover the entire digest.
    unsafe {
        _mm_storeu_si128(
            result.as_mut_ptr().cast(),
            _mm_shuffle_epi8(_mm_shuffle_epi32::<0x1b>(abcd), swap),
        );
        _mm_storeu_si128(
            result.as_mut_ptr().add(16).cast(),
            _mm_shuffle_epi8(_mm_shuffle_epi32::<0x1b>(efgh), swap),
        );
    }
    result
}

pub(super) fn accelerated(bytes: &[u8]) -> Option<[u8; 32]> {
    if is_x86_feature_detected!("sha")
        && is_x86_feature_detected!("ssse3")
        && is_x86_feature_detected!("sse4.1")
    {
        // SAFETY: all target features of hash have been checked.
        Some(unsafe { hash(bytes) })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn matches_portable_hash_across_padding_and_block_boundaries() {
        let data: Vec<_> = (0..4096).map(|i| (i * 31) as u8).collect();
        for n in (0..256).chain([2048, 4095, 4096]) {
            if let Some(sum) = super::accelerated(&data[..n]) {
                assert_eq!(sum, hmac_sha256::Hash::hash(&data[..n]));
            }
        }
    }
}
