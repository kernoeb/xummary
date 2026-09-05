//! Reads the X session out of a Chromium-family browser's cookie database.
//!
//! Chromium stores cookie values encrypted with AES-128-CBC. The key comes
//! from a per-browser password: the macOS keychain entry "<Browser> Safe
//! Storage", or on Linux the literal "peanuts" when no keyring is in use.

use anyhow::{anyhow, bail, Context, Result};
use aes::Aes128;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use hmac::Hmac;
use rusqlite::Connection;
use sha1::Sha1;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Keeps each profile's working copy in its own directory within one run.
static COPY_SEQ: AtomicU32 = AtomicU32::new(0);

type Aes128CbcDec = cbc::Decryptor<Aes128>;

const SALT: &[u8] = b"saltysalt";
const KEY_LEN: usize = 16;
const IV: [u8; 16] = [0x20; 16];

/// Chrome 130+ prepends a SHA-256 of the cookie's domain to the plaintext.
const INTEGRITY_PREFIX: usize = 32;

#[cfg(target_os = "macos")]
const PBKDF2_ITERS: u32 = 1003;
#[cfg(not(target_os = "macos"))]
const PBKDF2_ITERS: u32 = 1;

#[derive(Debug, Clone)]
pub struct Session {
    pub auth_token: String,
    pub ct0: String,
}

struct Browser {
    label: &'static str,
    /// Profile roots, relative to the platform's browser data directory.
    roots: &'static [&'static str],
    keychain_service: &'static str,
    keychain_account: &'static str,
}

#[cfg(target_os = "macos")]
const BROWSERS: &[Browser] = &[
    Browser {
        label: "chrome",
        roots: &["Google/Chrome", "Google/Chrome Beta", "Google/Chrome Canary"],
        keychain_service: "Chrome Safe Storage",
        keychain_account: "Chrome",
    },
    Browser {
        label: "brave",
        roots: &["BraveSoftware/Brave-Browser"],
        keychain_service: "Brave Safe Storage",
        keychain_account: "Brave",
    },
    Browser {
        label: "chromium",
        roots: &["Chromium"],
        keychain_service: "Chromium Safe Storage",
        keychain_account: "Chromium",
    },
    Browser {
        label: "edge",
        roots: &["Microsoft Edge"],
        keychain_service: "Microsoft Edge Safe Storage",
        keychain_account: "Microsoft Edge",
    },
    Browser {
        label: "vivaldi",
        roots: &["Vivaldi"],
        keychain_service: "Vivaldi Safe Storage",
        keychain_account: "Vivaldi",
    },
    Browser {
        label: "arc",
        roots: &["Arc/User Data"],
        keychain_service: "Arc Safe Storage",
        keychain_account: "Arc",
    },
];

#[cfg(not(target_os = "macos"))]
const BROWSERS: &[Browser] = &[
    Browser {
        label: "chrome",
        roots: &["google-chrome", "google-chrome-beta"],
        keychain_service: "",
        keychain_account: "",
    },
    Browser {
        label: "brave",
        roots: &["BraveSoftware/Brave-Browser"],
        keychain_service: "",
        keychain_account: "",
    },
    Browser {
        label: "chromium",
        roots: &["chromium"],
        keychain_service: "",
        keychain_account: "",
    },
    Browser {
        label: "vivaldi",
        roots: &["vivaldi"],
        keychain_service: "",
        keychain_account: "",
    },
];

/// Finds an X session in the first browser that has one. `only` restricts the
/// search to one browser label.
pub fn load(only: Option<&str>) -> Result<(Session, &'static str)> {
    let mut tried = Vec::new();
    let mut last_err = None;

    for browser in BROWSERS {
        if only.is_some_and(|w| !browser.label.eq_ignore_ascii_case(w)) {
            continue;
        }
        for db in profile_databases(browser) {
            tried.push(db.display().to_string());
            match session_from_db(&db, browser) {
                Ok(session) => return Ok((session, browser.label)),
                Err(e) => last_err = Some(e),
            }
        }
    }

    if tried.is_empty() {
        bail!(
            "no Chromium-family cookie database found{}. log into x.com in Chrome, Brave, \
             Chromium, Edge, Vivaldi or Arc first.",
            only.map(|w| format!(" for --browser {w}")).unwrap_or_default()
        );
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no x.com cookies in any browser profile")))
        .with_context(|| format!("looked in {} profile(s)", tried.len()))
}

/// Every `Cookies` file under a browser's profile directories.
fn profile_databases(browser: &Browser) -> Vec<PathBuf> {
    let Some(base) = data_root() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for root in browser.roots {
        let root = base.join(root);
        if !root.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut profiles: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        // "Default" first: it is the profile people actually use.
        profiles.sort_by_key(|p| p.file_name().map(|n| n != "Default").unwrap_or(true));
        for profile in profiles {
            for name in ["Cookies", "Network/Cookies"] {
                let db = profile.join(name);
                if db.is_file() {
                    out.push(db);
                }
            }
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn data_root() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().join("Library/Application Support"))
}

#[cfg(not(target_os = "macos"))]
fn data_root() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().to_path_buf())
}

fn session_from_db(db: &Path, browser: &Browser) -> Result<Session> {
    // The browser holds a write lock on the live file, so read a copy. The copy
    // goes in a directory of our own: `create_dir` fails if the path exists, so
    // nobody can plant a symlink there and redirect your cookies.
    let seq = COPY_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("xummary-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).with_context(|| format!("create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }

    let copy = dir.join("Cookies");
    let result = copy_and_read(db, &copy, browser);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn copy_and_read(db: &Path, copy: &Path, browser: &Browser) -> Result<Session> {
    std::fs::copy(db, copy).with_context(|| format!("copy {}", db.display()))?;
    // Recent writes live in the -wal sidecar. Without it a fresh login can read
    // as no cookies at all.
    let wal = db.with_file_name(format!(
        "{}-wal",
        db.file_name().unwrap_or_default().to_string_lossy()
    ));
    if wal.is_file() {
        let _ = std::fs::copy(&wal, copy.with_file_name("Cookies-wal"));
    }
    read_and_decrypt(copy, browser)
}

fn read_and_decrypt(db: &Path, browser: &Browser) -> Result<Session> {
    let conn = Connection::open(db).context("open cookie database")?;
    let mut stmt = conn.prepare(
        "SELECT host_key, name, value, encrypted_value FROM cookies \
         WHERE host_key IN ('x.com', '.x.com', 'twitter.com', '.twitter.com') \
           AND name IN ('auth_token', 'ct0')",
    )?;
    let mut rows: Vec<(String, String, String, Vec<u8>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;

    if rows.is_empty() {
        bail!("no x.com cookies in {}", db.display());
    }

    // The old twitter.com host can still hold a dead session, and SQLite returns
    // rows in no particular order, so take x.com first and keep the first hit.
    rows.sort_by_key(|(host, ..)| !host.ends_with("x.com"));

    let keys = candidate_keys(browser);
    let mut auth_token: Option<String> = None;
    let mut ct0: Option<String> = None;
    let mut last_err = None;

    for (_, name, plain, encrypted) in rows {
        let slot = match name.as_str() {
            "auth_token" => &mut auth_token,
            "ct0" => &mut ct0,
            _ => continue,
        };
        if slot.is_some() {
            continue;
        }
        let value = if !plain.is_empty() {
            plain
        } else {
            // One unreadable row must not sink a profile whose other rows are fine.
            match decrypt(&encrypted, &keys) {
                Ok(value) => value,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            }
        };
        if !value.is_empty() {
            *slot = Some(value);
        }
    }

    match (auth_token, ct0) {
        (Some(auth_token), Some(ct0)) => Ok(Session { auth_token, ct0 }),
        _ => match last_err {
            Some(e) => Err(e).context("could not read the x.com cookies"),
            None => bail!("found x.com cookies but not both auth_token and ct0 — log in again"),
        },
    }
}

/// One key per password the browser might have used. macOS reads the keychain;
/// elsewhere Chromium falls back to a fixed password when there is no keyring.
fn candidate_keys(browser: &Browser) -> Vec<[u8; KEY_LEN]> {
    passwords(browser)
        .iter()
        .filter_map(|pw| {
            let mut key = [0u8; KEY_LEN];
            pbkdf2::pbkdf2::<Hmac<Sha1>>(pw, SALT, PBKDF2_ITERS, &mut key).ok()?;
            Some(key)
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn passwords(browser: &Browser) -> Vec<Vec<u8>> {
    match security_framework::passwords::get_generic_password(
        browser.keychain_service,
        browser.keychain_account,
    ) {
        Ok(pw) if !pw.is_empty() => vec![pw],
        _ => Vec::new(),
    }
}

#[cfg(not(target_os = "macos"))]
fn passwords(_browser: &Browser) -> Vec<Vec<u8>> {
    vec![b"peanuts".to_vec()]
}

fn decrypt(encrypted: &[u8], keys: &[[u8; KEY_LEN]]) -> Result<String> {
    if encrypted.len() < 3 {
        bail!("cookie value is empty");
    }
    if keys.is_empty() {
        bail!("could not read the browser's cookie password (keychain access denied?)");
    }
    // v10/v11 mark the AES-CBC scheme; anything else is a format we cannot read.
    let prefix = &encrypted[..3];
    if prefix != b"v10" && prefix != b"v11" {
        bail!("unsupported cookie encryption {:?}", String::from_utf8_lossy(prefix));
    }

    for key in keys {
        if let Ok(value) = decrypt_with(&encrypted[3..], key) {
            return Ok(value);
        }
    }
    bail!("cookie decryption failed with every candidate key")
}

fn decrypt_with(body: &[u8], key: &[u8; KEY_LEN]) -> Result<String> {
    if body.is_empty() || body.len() % 16 != 0 {
        bail!("ciphertext is not a whole number of AES blocks");
    }
    let mut buf = body.to_vec();
    let plain = Aes128CbcDec::new(key.into(), &IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| anyhow!("AES-CBC unpad failed"))?;

    if is_printable(plain) {
        return Ok(String::from_utf8_lossy(plain).into_owned());
    }
    // Newer Chrome hides the value behind a domain hash.
    if plain.len() > INTEGRITY_PREFIX && is_printable(&plain[INTEGRITY_PREFIX..]) {
        return Ok(String::from_utf8_lossy(&plain[INTEGRITY_PREFIX..]).into_owned());
    }
    bail!("decrypted bytes are not a printable cookie value")
}

fn is_printable(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|b| (0x20..0x7f).contains(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncryptMut;

    type Aes128CbcEnc = cbc::Encryptor<Aes128>;

    fn encrypt(value: &[u8], key: &[u8; KEY_LEN]) -> Vec<u8> {
        let mut buf = vec![0u8; value.len() + 16];
        buf[..value.len()].copy_from_slice(value);
        let ct = Aes128CbcEnc::new(key.into(), &IV.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buf, value.len())
            .unwrap()
            .to_vec();
        let mut out = b"v10".to_vec();
        out.extend_from_slice(&ct);
        out
    }

    fn key_for(password: &[u8]) -> [u8; KEY_LEN] {
        let mut key = [0u8; KEY_LEN];
        pbkdf2::pbkdf2::<Hmac<Sha1>>(password, SALT, PBKDF2_ITERS, &mut key).unwrap();
        key
    }

    #[test]
    fn round_trips_a_plain_value() {
        let key = key_for(b"peanuts");
        let blob = encrypt(b"abc123", &key);
        assert_eq!(decrypt(&blob, &[key]).unwrap(), "abc123");
    }

    #[test]
    fn strips_the_domain_hash_prefix() {
        let key = key_for(b"peanuts");
        let mut value = vec![0xABu8; INTEGRITY_PREFIX];
        value.extend_from_slice(b"abc123");
        let blob = encrypt(&value, &key);
        assert_eq!(decrypt(&blob, &[key]).unwrap(), "abc123");
    }

    #[test]
    fn rejects_a_wrong_key() {
        let blob = encrypt(b"abc123", &key_for(b"peanuts"));
        assert!(decrypt(&blob, &[key_for(b"wrong")]).is_err());
    }
}
