use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// Pure Rust SHA-256 implementation (FIPS 180-4 compliant)
pub struct Sha256 {
    state: [u32; 8],
    count: u64,
    buffer: [u8; 64],
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
            ],
            count: 0,
            buffer: [0u8; 64],
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut index = (self.count % 64) as usize;
        self.count += data.len() as u64;

        let mut data_offset = 0;
        let mut data_len = data.len();

        if index > 0 && index + data_len >= 64 {
            let fill = 64 - index;
            self.buffer[index..64].copy_from_slice(&data[..fill]);
            let block = self.buffer;
            self.transform(&block);
            data_offset += fill;
            data_len -= fill;
            index = 0;
        }

        while data_len >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[data_offset..data_offset + 64]);
            self.transform(&block);
            data_offset += 64;
            data_len -= 64;
        }

        if data_len > 0 {
            self.buffer[index..index + data_len].copy_from_slice(&data[data_offset..]);
        }
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let total_bits = self.count * 8;
        let index = (self.count % 64) as usize;

        // Append padding 0x80 byte
        self.buffer[index] = 0x80;

        if index < 56 {
            // Pad remaining bytes up to index 55 with 0
            for b in &mut self.buffer[index + 1..56] {
                *b = 0;
            }
        } else {
            // Pad remaining bytes in current block with 0
            for b in &mut self.buffer[index + 1..64] {
                *b = 0;
            }
            let block = self.buffer;
            self.transform(&block);
            // New block initialized to zeros
            self.buffer = [0u8; 64];
        }

        // Place length in bits as 64-bit big endian integer in last 8 bytes
        self.buffer[56..64].copy_from_slice(&total_bits.to_be_bytes());
        let block = self.buffer;
        self.transform(&block);


        let mut out = [0u8; 32];
        for (i, val) in self.state.iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&val.to_be_bytes());
        }
        out
    }

    fn transform(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];
        let mut e = self.state[4];
        let mut f = self.state[5];
        let mut g = self.state[6];
        let mut h = self.state[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

pub fn sha256_hex(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    let hash = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in hash {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", byte);
    }
    hex
}

/// Compares two byte slices in constant time to prevent timing side-channel attacks.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Compares two strings in constant time to prevent timing side-channel attacks.
pub fn constant_time_eq_str(a: &str, b: &str) -> bool {
    constant_time_eq(a.as_bytes(), b.as_bytes())
}

/// Computes HMAC-SHA256 (RFC 2104 / FIPS 198-1 compliant)
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let mut hasher = Sha256::new();
        hasher.update(key);
        let key_hash = hasher.finalize();
        k[..32].copy_from_slice(&key_hash);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0u8; 64];
    let mut opad = [0u8; 64];
    for i in 0..64 {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
    }

    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize()
}

/// Derives a 32-byte key using PBKDF2-HMAC-SHA256 (RFC 2898 / RFC 8018)
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut salt_with_index = Vec::with_capacity(salt.len() + 4);
    salt_with_index.extend_from_slice(salt);
    salt_with_index.extend_from_slice(&1u32.to_be_bytes());

    let mut u = hmac_sha256(password, &salt_with_index);
    let mut result = u;

    for _ in 1..iterations {
        u = hmac_sha256(password, &u);
        for i in 0..32 {
            result[i] ^= u[i];
        }
    }

    result
}

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generates a cryptographically random 32-character hex salt using OS entropy
pub fn generate_secure_salt() -> String {
    let u1 = uuid::Uuid::new_v4();
    let u2 = uuid::Uuid::new_v4();
    sha256_hex(format!("{}:{}", u1, u2).as_bytes())[..32].to_string()
}

pub const PBKDF2_DEFAULT_ITERATIONS: u32 = 20_000;

/// Hashes password with PBKDF2-HMAC-SHA256: returns "pbkdf2$iterations$salt$hash"
pub fn hash_password(password: &str, salt: Option<&str>) -> String {
    let active_salt = salt.map(|s| s.to_string()).unwrap_or_else(generate_secure_salt);
    let derived = pbkdf2_hmac_sha256(password.as_bytes(), active_salt.as_bytes(), PBKDF2_DEFAULT_ITERATIONS);

    let mut hex = String::with_capacity(64);
    for byte in derived {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", byte);
    }
    format!("pbkdf2${}${}${}", PBKDF2_DEFAULT_ITERATIONS, active_salt, hex)
}

/// Verifies a password against stored hash using constant-time comparison.
/// Supports modern "pbkdf2$iterations$salt$hash" and backwards-compatible "salt$hash".
pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    let parts: Vec<&str> = stored_hash.split('$').collect();
    if parts.len() == 4 && parts[0] == "pbkdf2" {
        let iterations: u32 = parts[1].parse().unwrap_or(PBKDF2_DEFAULT_ITERATIONS);
        let salt = parts[2];
        let expected_hex = parts[3];
        let derived = pbkdf2_hmac_sha256(password.as_bytes(), salt.as_bytes(), iterations);
        let mut derived_hex = String::with_capacity(64);
        for byte in derived {
            use std::fmt::Write;
            let _ = write!(derived_hex, "{:02x}", byte);
        }
        constant_time_eq_str(&derived_hex, expected_hex)
    } else if parts.len() == 2 {
        // Backwards compatibility with legacy "salt$hash"
        let salt = parts[0];
        let expected = parts[1];
        let combined = format!("{}:{}", salt, password);
        let digest = sha256_hex(combined.as_bytes());
        constant_time_eq_str(&digest, expected)
    } else {
        false
    }
}

/// Generates a cryptographically secure session token using OS entropy and monotonic counter
pub fn generate_session_token(secret: &str) -> String {
    let u = uuid::Uuid::new_v4();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let counter = TOKEN_COUNTER.fetch_add(1, Ordering::SeqCst);
    let raw = format!("session-{}-{}-{}-{}", secret, u, now, counter);
    sha256_hex(raw.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        for len in [0, 1, 55, 56, 57, 63, 64, 65, 127, 128, 256] {
            let data = vec![b'a'; len];
            let hash = sha256_hex(&data);
            assert_eq!(hash.len(), 64);
        }
    }

    #[test]
    fn test_hmac_sha256_rfc4231() {
        // RFC 4231 Test Case 2: key="Jefe", data="what do ya want for nothing?"
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let hmac = hmac_sha256(key, data);
        let mut hex = String::new();
        for b in hmac {
            use std::fmt::Write;
            let _ = write!(hex, "{:02x}", b);
        }
        assert_eq!(hex, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn test_pbkdf2_sha256_standard_vectors() {
        // Standard PBKDF2-HMAC-SHA256: P="password", S="salt", c=1, dkLen=32
        let p = b"password";
        let s = b"salt";
        let dk1 = pbkdf2_hmac_sha256(p, s, 1);
        let mut hex1 = String::new();
        for b in dk1 {
            use std::fmt::Write;
            let _ = write!(hex1, "{:02x}", b);
        }
        assert_eq!(hex1, "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");

        // Standard PBKDF2-HMAC-SHA256: P="password", S="salt", c=2, dkLen=32
        let dk2 = pbkdf2_hmac_sha256(p, s, 2);
        let mut hex2 = String::new();
        for b in dk2 {
            use std::fmt::Write;
            let _ = write!(hex2, "{:02x}", b);
        }
        assert_eq!(hex2, "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43");
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"hellp"));
        assert!(!constant_time_eq(b"hello", b"hell"));
        assert!(constant_time_eq_str("secret-token-12345", "secret-token-12345"));
        assert!(!constant_time_eq_str("secret-token-12345", "secret-token-12346"));
    }

    #[test]
    fn test_hash_and_verify_password_pbkdf2() {
        let pwd = "SuperSecretPassword123!";
        let hash = hash_password(pwd, None);
        assert!(hash.starts_with("pbkdf2$20000$"));
        assert!(verify_password(pwd, &hash));
        assert!(!verify_password("WrongPassword", &hash));
    }

    #[test]
    fn test_verify_password_legacy_hash_compatibility() {
        // Test backwards compatibility with legacy "salt$hash"
        let pwd = "OldLegacyPassword!";
        let salt = "mysalt1234567890";
        let combined = format!("{}:{}", salt, pwd);
        let legacy_hash = format!("{}${}", salt, sha256_hex(combined.as_bytes()));

        assert!(verify_password(pwd, &legacy_hash));
        assert!(!verify_password("IncorrectPassword", &legacy_hash));
    }

    #[test]
    fn test_generate_session_token() {
        let token1 = generate_session_token("my-secret");
        let token2 = generate_session_token("my-secret");
        assert_eq!(token1.len(), 64);
        assert_eq!(token2.len(), 64);
        assert_ne!(token1, token2); // Different tokens due to UUID/counter
    }
}


