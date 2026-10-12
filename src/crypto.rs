//! SimpleTextCrypt 5.x compatible encryption (a bit-exact port of the simplecrypter Go package).
//!
//! Format (do not change: the browser, CLI and Android versions must read our messages):
//! - IV: 12 random bytes, both the AES-GCM nonce and the Argon2id salt.
//! - Key: Argon2id v0x13, t=5, m=65536 KiB, p=2, 32 bytes; the password as raw UTF-8 bytes.
//! - AES-256-GCM, 128-bit tag appended to the ciphertext, no AAD.
//! - Text: Base64(IV) ‖ Base64(ciphertext‖tag) in Android's Base64.DEFAULT style: wrapped
//!   every 76 characters, '\n' after each line including the last.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD};
use zeroize::Zeroizing;

const IV_LEN: usize = 12;
const TAG_LEN: usize = 16;
const LINE_WIDTH: usize = 76;
/// Base64 length of the IV.
const IV_B64_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidFormat,
    DecryptFailed,
    EmptyPassword,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::InvalidFormat => "not an encrypted message",
            Error::DecryptFailed => "wrong password or corrupted message",
            Error::EmptyPassword => "password must not be empty",
        })
    }
}

impl std::error::Error for Error {}

/// Encrypts `text` with a random IV into the text form.
pub fn encrypt(password: &str, text: &str) -> Result<String, Error> {
    let mut iv = [0u8; IV_LEN];
    getrandom::fill(&mut iv).expect("OS random number generator failed");
    encrypt_with_iv(password, text, iv)
}

/// Deterministic encryption, for test vectors only: never reuse an IV in production.
#[doc(hidden)]
pub fn encrypt_with_iv(password: &str, text: &str, iv: [u8; IV_LEN]) -> Result<String, Error> {
    let sealed = cipher(password, &iv)?.encrypt(Nonce::from_slice(&iv), text.as_bytes()).map_err(|_| Error::DecryptFailed)?;
    Ok(android_base64(&iv) + &android_base64(&sealed))
}

/// Decrypts the text form. All whitespace is ignored: messages travel through chats.
pub fn decrypt(password: &str, message: &str) -> Result<String, Error> {
    let compact = compact_whitespace(message);
    if compact.len() <= IV_B64_LEN {
        return Err(Error::InvalidFormat);
    }
    let (iv, sealed) = compact.as_bytes().split_at(IV_B64_LEN);
    let (iv, sealed) = (decode_base64(iv)?, decode_base64(sealed)?);
    if iv.len() != IV_LEN || sealed.len() < TAG_LEN {
        return Err(Error::InvalidFormat);
    }
    let plain = cipher(password, &iv)?.decrypt(Nonce::from_slice(&iv), sealed.as_slice()).map_err(|_| Error::DecryptFailed)?;
    String::from_utf8(plain).map_err(|_| Error::DecryptFailed)
}

fn cipher(password: &str, iv: &[u8]) -> Result<Aes256Gcm, Error> {
    if password.is_empty() {
        return Err(Error::EmptyPassword);
    }
    let params = Params::new(65536, 5, 2, Some(32)).expect("valid Argon2 params");
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), iv, key.as_mut())
        .expect("Argon2 accepts a 12-byte salt");
    Ok(Aes256Gcm::new(key.as_ref().into()))
}

/// Android's Base64.DEFAULT: 76-character lines, each ending in '\n'.
fn android_base64(data: &[u8]) -> String {
    let encoded = STANDARD.encode(data);
    let mut out = String::with_capacity(encoded.len() + encoded.len() / LINE_WIDTH + 1);
    for line in encoded.as_bytes().chunks(LINE_WIDTH) {
        out.push_str(std::str::from_utf8(line).expect("Base64 is ASCII"));
        out.push('\n');
    }
    out
}

/// Accepts missing padding and non-zero trailing bits, like Go's RawStdEncoding and atob().
fn decode_base64(s: &[u8]) -> Result<Vec<u8>, Error> {
    const LENIENT: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::RequireNone).with_decode_allow_trailing_bits(true),
    );
    let end = s.iter().rposition(|&b| b != b'=').map_or(0, |i| i + 1);
    LENIENT.decode(&s[..end]).map_err(|_| Error::InvalidFormat)
}

/// The same whitespace set as the Go version's CompactWhitespace.
fn compact_whitespace(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0B' | '\x0C' | '\u{85}' | '\u{A0}')).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANDROID_MESSAGE: &str = "+B8950QofZjSTcB0\no8rT3U6yY0CSmobkU1aMW3cKHH/ObMsxi5qDGQHHy580pj4tTQ==";
    const ANDROID_PASSWORD: &str = "aurinkoistapäivääteillekaikille";

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Vector {
        password: String,
        plaintext: String,
        iv_hex: String,
        message: String,
    }

    fn vectors() -> Vec<Vector> {
        let v: Vec<Vector> = serde_json::from_str(include_str!("../tests/data/simpletextcrypt-vectors.json")).unwrap();
        assert!(!v.is_empty());
        v
    }

    fn iv(hex: &str) -> [u8; 12] {
        let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        bytes.try_into().unwrap()
    }

    #[test]
    fn vectors_encrypt_bit_exact() {
        for v in vectors() {
            assert_eq!(encrypt_with_iv(&v.password, &v.plaintext, iv(&v.iv_hex)).unwrap(), v.message);
        }
    }

    #[test]
    fn vectors_decrypt() {
        for v in vectors() {
            assert_eq!(decrypt(&v.password, &v.message).unwrap(), v.plaintext);
        }
    }

    #[test]
    fn real_android_message_decrypts() {
        assert_eq!(decrypt(ANDROID_PASSWORD, ANDROID_MESSAGE).unwrap(), "Juankoski here i come");
    }

    #[test]
    fn whitespace_is_ignored() {
        // The Android message after a chat turned its newlines into spaces.
        let mangled = "  +B8950Qof ZjSTcB0 o8rT3U6yY0CSmobkU1aMW3cKHH/O\r\n bMsxi5qDGQHHy580pj4tTQ==  \n";
        assert_eq!(decrypt(ANDROID_PASSWORD, mangled).unwrap(), "Juankoski here i come");
        // Missing padding is fine too.
        assert_eq!(decrypt(ANDROID_PASSWORD, ANDROID_MESSAGE.trim_end_matches('=')).unwrap(), "Juankoski here i come");
    }

    #[test]
    fn random_iv_round_trip_and_wrapping() {
        let text = "Hei maailma, ääkköset ja 😜".repeat(10);
        let (a, b) = (encrypt("password", &text).unwrap(), encrypt("password", &text).unwrap());
        assert_ne!(a, b);
        assert_eq!(decrypt("password", &a).unwrap(), text);
        let lines: Vec<&str> = a.strip_suffix('\n').unwrap().split('\n').collect();
        assert_eq!(lines[0].len(), 16);
        assert!(lines[1..].iter().all(|l| l.len() <= 76) && lines.len() > 2);
    }

    #[test]
    fn errors() {
        let msg = encrypt("correct", "secret").unwrap();
        assert_eq!(decrypt("wrong", &msg), Err(Error::DecryptFailed));
        assert_eq!(encrypt("", "text"), Err(Error::EmptyPassword));
        assert_eq!(decrypt("", &msg), Err(Error::EmptyPassword));
        for bad in ["", "hei maailma", "AAAA", &"A".repeat(16), "äääääääääääääääää", "AAAAAAAAAAAAAAAA!!!!"] {
            assert_eq!(decrypt("pw", bad), Err(Error::InvalidFormat), "{bad:?}");
        }
    }
}
