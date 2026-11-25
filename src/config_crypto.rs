//! Pure-Rust authenticated encryption (ChaCha20 + HMAC-SHA256) for `micropdf.ini`.
//!
//! Zero external Cargo dependencies: implements RFC 7539 ChaCha20, FIPS 180-4 SHA-256,
//! RFC 2104 HMAC-SHA256, and RFC 4648 Base64 directly in ~220 lines of Rust.
//!
//! Encrypted payloads use the prefix `MPDFCFG1:` followed by Base64 of:
//! `12-byte random nonce || 32-byte HMAC-SHA256(mac_key, nonce || ciphertext) || ciphertext`.

use anyhow::{Result, bail};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const ENCRYPTED_PREFIX: &str = "MPDFCFG1:";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 32;

/// Application-derived master secret so MicroPDF opens encrypted configs seamlessly
/// across any USB drive or PC without prompting the user for a password.
const APP_MASTER_SEED: &[u8] = b"MicroPDF-Portable-Config-v1.7.0-ChaCha20-HMAC-SHA256-Key";

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(crate) fn is_encrypted_config(raw: &str) -> bool {
    raw.trim_start().starts_with(ENCRYPTED_PREFIX)
}

pub(crate) fn encrypt_config(plain: &str) -> String {
    let (enc_key, mac_key) = derive_keys();
    let nonce = generate_nonce();
    let mut ciphertext = plain.as_bytes().to_vec();
    chacha20_xor_in_place(&enc_key, &nonce, 1, &mut ciphertext);

    let tag = compute_tag(&mac_key, &nonce, &ciphertext);

    let mut blob = Vec::with_capacity(NONCE_LEN + TAG_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&tag);
    blob.extend_from_slice(&ciphertext);

    let mut out = String::with_capacity(ENCRYPTED_PREFIX.len() + (blob.len() * 4) / 3 + 4);
    out.push_str(ENCRYPTED_PREFIX);
    out.push_str(&base64_encode(&blob));
    out.push('\n');
    out
}

pub(crate) fn decrypt_config(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    let Some(b64) = trimmed.strip_prefix(ENCRYPTED_PREFIX) else {
        bail!("Missing encrypted config prefix");
    };
    let blob = base64_decode(b64.trim())?;
    if blob.len() < NONCE_LEN + TAG_LEN {
        bail!("Encrypted config is truncated");
    }
    let nonce: [u8; NONCE_LEN] = blob[..NONCE_LEN].try_into().unwrap();
    let tag: [u8; TAG_LEN] = blob[NONCE_LEN..NONCE_LEN + TAG_LEN].try_into().unwrap();
    let mut plaintext = blob[NONCE_LEN + TAG_LEN..].to_vec();

    let (enc_key, mac_key) = derive_keys();
    let expected_tag = compute_tag(&mac_key, &nonce, &plaintext);
    if !constant_time_eq(&tag, &expected_tag) {
        bail!("Encrypted config failed integrity verification");
    }

    chacha20_xor_in_place(&enc_key, &nonce, 1, &mut plaintext);
    String::from_utf8(plaintext).map_err(|_| anyhow::anyhow!("Decrypted config is not valid UTF-8"))
}

fn derive_keys() -> ([u8; 32], [u8; 32]) {
    let enc_key = hmac_sha256(APP_MASTER_SEED, b"enc-key-v1");
    let mac_key = hmac_sha256(APP_MASTER_SEED, b"mac-key-v1");
    (enc_key, mac_key)
}

fn compute_tag(mac_key: &[u8; 32], nonce: &[u8; NONCE_LEN], ciphertext: &[u8]) -> [u8; TAG_LEN] {
    let mut msg = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    msg.extend_from_slice(nonce);
    msg.extend_from_slice(ciphertext);
    hmac_sha256(mac_key, &msg)
}

fn generate_nonce() -> [u8; NONCE_LEN] {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut seed = Vec::with_capacity(28);
    seed.extend_from_slice(&nanos.to_le_bytes());
    seed.extend_from_slice(&seq.to_le_bytes());
    seed.extend_from_slice(&pid.to_le_bytes());
    let digest = sha256(&seed);
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&digest[..NONCE_LEN]);
    nonce
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// RFC 7539 ChaCha20
// ---------------------------------------------------------------------------

#[inline]
fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[0] = 0x6170_7865;
    state[1] = 0x3320_646e;
    state[2] = 0x7962_2d32;
    state[3] = 0x6b20_6574;
    for i in 0..8 {
        state[4 + i] = u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
    }
    state[12] = counter;
    for i in 0..3 {
        state[13 + i] = u32::from_le_bytes(nonce[i * 4..i * 4 + 4].try_into().unwrap());
    }

    let mut working = state;
    for _ in 0..10 {
        quarter_round(&mut working, 0, 4, 8, 12);
        quarter_round(&mut working, 1, 5, 9, 13);
        quarter_round(&mut working, 2, 6, 10, 14);
        quarter_round(&mut working, 3, 7, 11, 15);
        quarter_round(&mut working, 0, 5, 10, 15);
        quarter_round(&mut working, 1, 6, 11, 12);
        quarter_round(&mut working, 2, 7, 8, 13);
        quarter_round(&mut working, 3, 4, 9, 14);
    }

    let mut out = [0u8; 64];
    for i in 0..16 {
        let word = working[i].wrapping_add(state[i]).to_le_bytes();
        out[i * 4..i * 4 + 4].copy_from_slice(&word);
    }
    out
}

fn chacha20_xor_in_place(key: &[u8; 32], nonce: &[u8; 12], mut counter: u32, data: &mut [u8]) {
    for chunk in data.chunks_mut(64) {
        let block = chacha20_block(key, counter, nonce);
        for (b, k) in chunk.iter_mut().zip(block.iter()) {
            *b ^= *k;
        }
        counter = counter.wrapping_add(1);
    }
}

// ---------------------------------------------------------------------------
// FIPS 180-4 SHA-256 & RFC 2104 HMAC-SHA256
// ---------------------------------------------------------------------------

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k_block = [0u8; 64];
    if key.len() > 64 {
        let hk = sha256(key);
        k_block[..32].copy_from_slice(&hk);
    } else {
        k_block[..key.len()].copy_from_slice(key);
    }

    let mut inner = Vec::with_capacity(64 + msg.len());
    for &b in &k_block {
        inner.push(b ^ 0x36);
    }
    inner.extend_from_slice(msg);
    let inner_hash = sha256(&inner);

    let mut outer = Vec::with_capacity(64 + 32);
    for &b in &k_block {
        outer.push(b ^ 0x5c);
    }
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

// ---------------------------------------------------------------------------
// RFC 4648 Base64
// ---------------------------------------------------------------------------

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() * 4).div_ceil(3));
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64_ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 { B64_ALPHABET[((n >> 6) & 0x3F) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64_ALPHABET[(n & 0x3F) as usize] as char } else { '=' });
    }
    out
}

fn base64_val(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if bytes.len() % 4 != 0 {
        bail!("Invalid Base64 length");
    }
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for chunk in bytes.chunks_exact(4) {
        let a = base64_val(chunk[0]).ok_or_else(|| anyhow::anyhow!("Invalid Base64 char"))?;
        let b = base64_val(chunk[1]).ok_or_else(|| anyhow::anyhow!("Invalid Base64 char"))?;
        let pad_c = chunk[2] == b'=';
        let pad_d = chunk[3] == b'=';
        let c = if pad_c { 0 } else { base64_val(chunk[2]).ok_or_else(|| anyhow::anyhow!("Invalid Base64 char"))? };
        let d = if pad_d { 0 } else { base64_val(chunk[3]).ok_or_else(|| anyhow::anyhow!("Invalid Base64 char"))? };
        let n = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6) | (d as u32);
        out.push(((n >> 16) & 0xFF) as u8);
        if !pad_c {
            out.push(((n >> 8) & 0xFF) as u8);
        }
        if !pad_d {
            out.push((n & 0xFF) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_crypto_roundtrip_and_tamper_rejection() {
        let plain = "ui_theme=dark\nui_scale=1.20\nrecent=C:\\Secret\\Report.pdf\n";
        let enc1 = encrypt_config(plain);
        let enc2 = encrypt_config(plain);

        assert!(is_encrypted_config(&enc1));
        assert!(!is_encrypted_config(plain));
        assert_ne!(enc1, enc2, "fresh nonce must make ciphertexts non-deterministic");
        assert!(!enc1.contains("Report.pdf"), "plaintext must not leak into encrypted config");

        let dec1 = decrypt_config(&enc1).expect("decrypt valid payload");
        let dec2 = decrypt_config(&enc2).expect("decrypt second payload");
        assert_eq!(dec1, plain);
        assert_eq!(dec2, plain);

        // Tampering any byte must fail HMAC verification
        let mut tampered_bytes = enc1.trim().as_bytes().to_vec();
        let last = tampered_bytes.len() - 2;
        tampered_bytes[last] = if tampered_bytes[last] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(tampered_bytes).unwrap();
        assert!(decrypt_config(&tampered).is_err());
    }
}
