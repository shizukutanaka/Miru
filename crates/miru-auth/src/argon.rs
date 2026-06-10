//! Memory-hard password hashing (Argon2id).
//!
//! Lesson learned from RustDesk CVE-2026-30785:
//! Insufficient computational cost in password hashing makes credentials
//! recoverable from a config file dump. We use Argon2id with parameters
//! tuned for ~100ms on a 2026-class CPU — too slow to brute-force at scale,
//! tolerable for one-time auth.
//!
//! For PIN hashing during pairing we still use PBKDF2-SHA256 because the
//! PIN is one-time and ephemeral; for any long-lived secret we use Argon2id.

use anyhow::Result;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, PasswordVerifier, SaltString},
    Algorithm, Argon2, Params, Version,
};
use zeroize::Zeroize;

/// OWASP-recommended Argon2id parameters as of 2024-2026.
/// 64MB memory, 3 iterations, 4-way parallelism.
const ARGON2_MEMORY_KB: u32 = 64 * 1024;
const ARGON2_ITERATIONS: u32 = 3;
const ARGON2_PARALLELISM: u32 = 4;

fn argon2() -> Argon2<'static> {
    let params = Params::new(
        ARGON2_MEMORY_KB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        Some(32),
    ).expect("argon2 params");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Hash a password into a PHC string (`$argon2id$v=19$...`).
/// Use this for any persisted credential.
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let phc = argon2()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 hash: {}", e))?
        .to_string();
    Ok(phc)
}

/// Verify a password against a PHC string. Constant-time.
pub fn verify_password(password: &str, phc: &str) -> Result<bool> {
    let parsed = argon2::PasswordHash::new(phc)
        .map_err(|e| anyhow::anyhow!("argon2 parse: {}", e))?;
    Ok(argon2().verify_password(password.as_bytes(), &parsed).is_ok())
}

/// Wrapper that auto-zeroes the secret on drop.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct SecretBytes(pub Vec<u8>);

impl SecretBytes {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self { Self(bytes.into()) }
    pub fn as_slice(&self) -> &[u8] { &self.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argon2_roundtrip() {
        let phc = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &phc).unwrap());
        assert!(!verify_password("wrong", &phc).unwrap());
    }

    #[test]
    fn argon2_different_salts() {
        // Same password, different hashes (salt differs)
        let h1 = hash_password("same").unwrap();
        let h2 = hash_password("same").unwrap();
        assert_ne!(h1, h2);
        // But both verify correctly
        assert!(verify_password("same", &h1).unwrap());
        assert!(verify_password("same", &h2).unwrap());
    }

    #[test]
    fn argon2_invalid_phc_rejected() {
        // A PHC parse failure → Err. A parse succeeds but verify fails → Ok(false).
        // Both are equivalent rejection signals; the test asserts neither yields Ok(true).
        let r1 = verify_password("any", "not-a-phc");
        assert!(r1.is_err() || matches!(r1, Ok(false)));
        let r2 = verify_password("any", "$argon2id$bogus");
        assert!(r2.is_err() || matches!(r2, Ok(false)));
    }
}
