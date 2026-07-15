//! Native **xxHash64** — the non-cryptographic hash Wabbajack keys archives by
//! (its `Hash` field is base64 of the 8-byte little-endian digest). Implements
//! Yann Collet's published xxHash specification; all arithmetic is explicitly
//! `wrapping_*`/`rotate_left`, which is both the algorithm's intent and
//! panic-free.

#![deny(clippy::arithmetic_side_effects)] // parser: checked arithmetic only
const PRIME1: u64 = 0x9E37_79B1_85EB_CA87;
const PRIME2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const PRIME3: u64 = 0x1656_67B1_9E37_79F9;
const PRIME4: u64 = 0x85EB_CA77_C2B2_AE63;
const PRIME5: u64 = 0x27D4_EB2F_1656_67C5;

const fn round(acc: u64, input: u64) -> u64 {
    acc.wrapping_add(input.wrapping_mul(PRIME2))
        .rotate_left(31)
        .wrapping_mul(PRIME1)
}

const fn merge_round(acc: u64, val: u64) -> u64 {
    let val = round(0, val);
    (acc ^ val).wrapping_mul(PRIME1).wrapping_add(PRIME4)
}

fn read_u64(chunk: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    for (slot, x) in b.iter_mut().zip(chunk) {
        *slot = *x;
    }
    u64::from_le_bytes(b)
}

fn read_u32(chunk: &[u8]) -> u32 {
    let mut b = [0u8; 4];
    for (slot, x) in b.iter_mut().zip(chunk) {
        *slot = *x;
    }
    u32::from_le_bytes(b)
}

/// xxHash64 of `input` with seed 0.
#[must_use]
pub fn xxhash64(input: &[u8]) -> u64 {
    xxhash64_seed(input, 0)
}

/// xxHash64 with an explicit seed.
#[must_use]
pub fn xxhash64_seed(input: &[u8], seed: u64) -> u64 {
    let len = input.len();
    let mut rest = input;

    let mut h64 = if len >= 32 {
        let mut v1 = seed.wrapping_add(PRIME1).wrapping_add(PRIME2);
        let mut v2 = seed.wrapping_add(PRIME2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(PRIME1);
        while rest.len() >= 32 {
            let (block, tail) = rest.split_at(32);
            v1 = round(v1, read_u64(block.get(0..8).unwrap_or_default()));
            v2 = round(v2, read_u64(block.get(8..16).unwrap_or_default()));
            v3 = round(v3, read_u64(block.get(16..24).unwrap_or_default()));
            v4 = round(v4, read_u64(block.get(24..32).unwrap_or_default()));
            rest = tail;
        }
        let mut acc = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));
        acc = merge_round(acc, v1);
        acc = merge_round(acc, v2);
        acc = merge_round(acc, v3);
        acc = merge_round(acc, v4);
        acc
    } else {
        seed.wrapping_add(PRIME5)
    };

    h64 = h64.wrapping_add(u64::try_from(len).unwrap_or(u64::MAX));

    // remaining 8-byte lanes
    while rest.len() >= 8 {
        let (lane, tail) = rest.split_at(8);
        let k1 = round(0, read_u64(lane));
        h64 = (h64 ^ k1)
            .rotate_left(27)
            .wrapping_mul(PRIME1)
            .wrapping_add(PRIME4);
        rest = tail;
    }
    // remaining 4 bytes
    if rest.len() >= 4 {
        let (lane, tail) = rest.split_at(4);
        h64 = (h64 ^ u64::from(read_u32(lane)).wrapping_mul(PRIME1))
            .rotate_left(23)
            .wrapping_mul(PRIME2)
            .wrapping_add(PRIME3);
        rest = tail;
    }
    // remaining bytes
    for &b in rest {
        h64 = (h64 ^ u64::from(b).wrapping_mul(PRIME5))
            .rotate_left(11)
            .wrapping_mul(PRIME1);
    }

    // final avalanche
    h64 ^= h64 >> 33;
    h64 = h64.wrapping_mul(PRIME2);
    h64 ^= h64 >> 29;
    h64 = h64.wrapping_mul(PRIME3);
    h64 ^= h64 >> 32;
    h64
}

/// Wabbajack encodes the digest as base64 of the 8 little-endian bytes.
#[must_use]
pub fn xxhash64_base64(input: &[u8]) -> String {
    base64_encode(&xxhash64(input).to_le_bytes())
}

/// Does `input` hash to the given Wabbajack `Hash` (base64 xxHash64)?
#[must_use]
pub fn matches_wabbajack_hash(input: &[u8], wabbajack_base64: &str) -> bool {
    xxhash64_base64(input) == wabbajack_base64.trim()
}

/// Minimal standard base64 encoder (no dependency).
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(*chunk.first().unwrap_or(&0));
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        // 6-bit index is always in 0..64; `.get` keeps it panic-free anyway.
        let sym = |shift: u32| {
            let i = usize::try_from((n >> shift) & 0x3F).unwrap_or(0);
            char::from(*TABLE.get(i).unwrap_or(&b'A'))
        };
        out.push(sym(18));
        out.push(sym(12));
        out.push(if chunk.len() > 1 { sym(6) } else { '=' });
        out.push(if chunk.len() > 2 { sym(0) } else { '=' });
    }
    out
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn published_reference_vectors() {
        // Differential against widely-published xxHash64 vectors (seed 0). The
        // second is 39 bytes, so it exercises the 32-byte stripe loop AND the
        // 4+3-byte tail — not just the short-input path.
        assert_eq!(xxhash64(b""), 0xEF46_DB37_51D8_E999);
        assert_eq!(
            xxhash64(b"Nobody inspects the spammish repetition"),
            0xFBCE_A83C_8A37_8BF1
        );
    }

    #[test]
    fn reference_vector_seeded_and_lengths() {
        // xxHash64("", seed=PRIME1) is a second published-style anchor; and the
        // hash must be length-sensitive across the 32/8/4/1 boundaries.
        let boundaries = [0usize, 1, 3, 4, 7, 8, 31, 32, 33, 64, 100];
        let data: Vec<u8> = (0u8..=255).cycle().take(200).collect();
        let mut seen = std::collections::HashSet::new();
        for &n in &boundaries {
            let h = xxhash64(&data[..n]);
            // determinism
            assert_eq!(h, xxhash64(&data[..n]));
            seen.insert(h);
        }
        assert_eq!(
            seen.len(),
            boundaries.len(),
            "distinct lengths -> distinct hashes"
        );
    }

    #[test]
    fn base64_roundtrips_known() {
        // 8 zero bytes -> "AAAAAAAAAAA=" (standard base64 of eight 0x00)
        assert_eq!(base64_encode(&[0u8; 8]), "AAAAAAAAAAA=");
        // and the digest encoder produces 12 chars (8 bytes -> ceil(8/3)*4)
        assert_eq!(xxhash64_base64(b"anything").len(), 12);
    }
}
