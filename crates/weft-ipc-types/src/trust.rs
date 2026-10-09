//! Package content identity, publisher keys and the trust decisions made
//! before a package is installed or launched.
//!
//! A package is signed over its content digest: the SHA-256 of a canonical
//! inventory listing every file except the root `signature.sig`, one
//! `<relative path>\t<hex sha-256>\n` line per file, sorted by path. Paths use
//! `/` and must be UTF-8 without control characters, so no name can imitate
//! the separators of other lines. Symbolic links and special files are
//! refused, so the signed bytes are exactly the files a package holds.

use std::fmt;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

/// The signature file at the root of a signed package.
pub const SIGNATURE_FILE: &str = "signature.sig";

/// Why a package's content or trust could not be established.
#[derive(Debug)]
pub enum TrustError {
    /// The package holds a symbolic link.
    Link(PathBuf),
    /// The package holds something other than directories and regular files.
    NotAFile(PathBuf),
    /// A path in the package is not UTF-8.
    NonUtf8(PathBuf),
    /// A name in the package holds a control character.
    ControlCharacter(PathBuf),
    /// A key, signature or owner record is malformed.
    Malformed(PathBuf, String),
    Io(PathBuf, std::io::Error),
}

impl fmt::Display for TrustError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Link(p) => write!(f, "{} is a symbolic link", p.display()),
            Self::NotAFile(p) => write!(f, "{} is not a regular file or directory", p.display()),
            Self::NonUtf8(p) => write!(f, "{} is not a UTF-8 path", p.display()),
            Self::ControlCharacter(p) => {
                write!(f, "{:?} has a control character in its name", p.display())
            }
            Self::Malformed(p, why) => write!(f, "{}: {why}", p.display()),
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for TrustError {}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> TrustError + '_ {
    move |e| TrustError::Io(path.to_path_buf(), e)
}

/// The SHA-256 content digest a package signature covers.
pub fn content_digest(package_root: &Path) -> Result<[u8; 32], TrustError> {
    let mut entries = Vec::new();
    collect(package_root, package_root, &mut entries)?;
    entries.sort();
    let mut inventory = String::new();
    for (path, hash) in &entries {
        inventory.push_str(path);
        inventory.push('\t');
        inventory.push_str(&hex::encode(hash));
        inventory.push('\n');
    }
    Ok(Sha256::digest(inventory.as_bytes()).into())
}

fn collect(
    root: &Path,
    dir: &Path,
    entries: &mut Vec<(String, [u8; 32])>,
) -> Result<(), TrustError> {
    for entry in std::fs::read_dir(dir).map_err(io(dir))? {
        let path = entry.map_err(io(dir))?.path();
        let relative = path
            .strip_prefix(root)
            .expect("read_dir yields children of the directory it reads")
            .to_str()
            .ok_or_else(|| TrustError::NonUtf8(path.clone()))?
            .to_owned();
        if relative.chars().any(char::is_control) {
            return Err(TrustError::ControlCharacter(path));
        }
        let kind = std::fs::symlink_metadata(&path)
            .map_err(io(&path))?
            .file_type();
        if kind.is_symlink() {
            return Err(TrustError::Link(path));
        } else if kind.is_dir() {
            collect(root, &path, entries)?;
        } else if !kind.is_file() {
            return Err(TrustError::NotAFile(path));
        } else if relative != SIGNATURE_FILE {
            let bytes = std::fs::read(&path).map_err(io(&path))?;
            entries.push((relative, Sha256::digest(&bytes).into()));
        }
    }
    Ok(())
}

/// A publisher's Ed25519 public key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublisherKey(VerifyingKey);

impl fmt::Debug for PublisherKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublisherKey({})", self.to_hex())
    }
}

impl PublisherKey {
    /// Parses 64 hex digits, the format `weft-pack generate-key` writes.
    pub fn from_hex(text: &str) -> Result<Self, String> {
        let bytes: [u8; 32] = hex::decode(text.trim())
            .map_err(|_| "expected 64 hex digits".to_owned())?
            .try_into()
            .map_err(|_| "expected a 32-byte key".to_owned())?;
        VerifyingKey::from_bytes(&bytes)
            .map(Self)
            .map_err(|_| "not a valid Ed25519 public key".to_owned())
    }

    pub fn from_file(path: &Path) -> Result<Self, TrustError> {
        let text = std::fs::read_to_string(path).map_err(io(path))?;
        Self::from_hex(&text).map_err(|why| TrustError::Malformed(path.to_path_buf(), why))
    }

    /// The key as 64 lowercase hex digits; it also identifies the publisher.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0.as_bytes())
    }

    /// Whether `signature` signs `digest` with this key.
    pub fn signed(&self, digest: &[u8; 32], signature: &Signature) -> bool {
        self.0.verify_strict(digest, signature).is_ok()
    }
}

/// Reads the package's signature, or `None` when the package is unsigned.
pub fn read_signature(package_root: &Path) -> Result<Option<Signature>, TrustError> {
    let path = package_root.join(SIGNATURE_FILE);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(TrustError::Io(path, e)),
        Ok(m) if !m.file_type().is_file() => return Err(TrustError::NotAFile(path)),
        Ok(_) => {}
    }
    let text = std::fs::read_to_string(&path).map_err(io(&path))?;
    let bytes: [u8; 64] = hex::decode(text.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| TrustError::Malformed(path, "expected 128 hex digits".to_owned()))?;
    Ok(Some(Signature::from_bytes(&bytes)))
}

/// The publisher keys this host trusts to sign packages.
#[derive(Debug, Default)]
pub struct TrustStore {
    keys: Vec<PublisherKey>,
}

impl TrustStore {
    /// The trust store directories, in order: `$WEFT_TRUSTED_KEYS` alone when
    /// set; otherwise the user's `$XDG_CONFIG_HOME/weft/trusted-keys` (or
    /// `~/.config/weft/trusted-keys`) and the system `/etc/weft/trusted-keys`.
    pub fn directories() -> Vec<PathBuf> {
        if let Some(dir) = std::env::var_os("WEFT_TRUSTED_KEYS") {
            return vec![PathBuf::from(dir)];
        }
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|home| home.is_absolute())
                    .map(|home| home.join(".config"))
            });
        let mut dirs: Vec<PathBuf> = config
            .map(|dir| dir.join("weft/trusted-keys"))
            .into_iter()
            .collect();
        dirs.push(PathBuf::from("/etc/weft/trusted-keys"));
        dirs
    }

    /// Loads every `*.pub` file in `directories`. A missing directory holds no
    /// keys; a malformed key file is an error rather than silently ignored.
    pub fn load(directories: &[PathBuf]) -> Result<Self, TrustError> {
        let mut keys = Vec::new();
        for dir in directories {
            let entries = match std::fs::read_dir(dir) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                other => other.map_err(io(dir))?,
            };
            let mut files: Vec<PathBuf> = entries
                .map(|e| e.map(|e| e.path()).map_err(io(dir)))
                .collect::<Result<_, _>>()?;
            files.sort();
            for file in files
                .into_iter()
                .filter(|f| f.extension().is_some_and(|e| e == "pub"))
            {
                keys.push(PublisherKey::from_file(&file)?);
            }
        }
        Ok(Self { keys })
    }

    pub fn keys(&self) -> &[PublisherKey] {
        &self.keys
    }

    /// The trusted key that signed the package at `package_root`, if any.
    pub fn signer(&self, package_root: &Path) -> Result<Option<PublisherKey>, TrustError> {
        let Some(signature) = read_signature(package_root)? else {
            return Ok(None);
        };
        let digest = content_digest(package_root)?;
        Ok(self
            .keys
            .iter()
            .copied()
            .find(|k| k.signed(&digest, &signature)))
    }
}

/// Who an application ID belongs to on this host. The record outlives the
/// package while the app's data remains, so another publisher cannot take
/// over the ID, and the data with it, by reusing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// Installed from packages signed by this trusted key.
    Verified(PublisherKey),
    /// Installed as development content, without a trusted signature.
    Development,
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Verified(key) => write!(f, "publisher {}", key.to_hex()),
            Self::Development => f.write_str("development installation"),
        }
    }
}

/// Where the owner of `app_id` is recorded, beside the app's data. Callers
/// pass a valid app ID, which cannot leave the owners directory.
pub fn owner_record_path(data_home: &Path, app_id: &str) -> PathBuf {
    data_home.join("weft/owners").join(app_id)
}

/// Reads an owner record: `verified <key>` or `development`.
pub fn read_owner(record: &Path) -> Result<Option<Owner>, TrustError> {
    let text = match std::fs::read_to_string(record) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        other => other.map_err(io(record))?,
    };
    let malformed = |why: &str| TrustError::Malformed(record.to_path_buf(), why.to_owned());
    match text.trim().split_once(' ') {
        Some(("verified", key)) => PublisherKey::from_hex(key)
            .map(|k| Some(Owner::Verified(k)))
            .map_err(|why| malformed(&why)),
        None if text.trim() == "development" => Ok(Some(Owner::Development)),
        _ => Err(malformed("expected 'verified <key>' or 'development'")),
    }
}

/// Records the owner of an app ID. An existing record is never replaced, and
/// a record is either absent or complete: it is written and synced under a
/// temporary name, then linked into place.
pub fn write_owner(record: &Path, owner: Owner) -> Result<(), TrustError> {
    use std::io::Write;
    let parent = record.parent().expect("owner records live in a directory");
    std::fs::create_dir_all(parent).map_err(io(parent))?;
    let text = match owner {
        Owner::Verified(key) => format!("verified {}\n", key.to_hex()),
        Owner::Development => "development\n".to_owned(),
    };
    let name = record
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("record");
    let temp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })()
    .map_err(io(&temp));
    let linked = written.and_then(|()| std::fs::hard_link(&temp, record).map_err(io(record)));
    let _ = std::fs::remove_file(&temp);
    linked?;
    std::fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(io(parent))
}

/// Takes the lock that serialises every change to the installation and
/// owner record of `app_id`, held until the returned file is dropped.
pub fn lock_owner(data_home: &Path, app_id: &str) -> Result<std::fs::File, TrustError> {
    let dir = data_home.join("weft/owners");
    if !crate::package::is_valid_app_id(app_id) {
        return Err(TrustError::Malformed(
            dir,
            format!("'{app_id}' is not a valid app ID"),
        ));
    }
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let path = dir.join(format!(".{app_id}.lock"));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(io(&path))?;
    file.lock().map_err(io(&path))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weft_trust_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(dir.join("wapp.toml"), b"[package]\n").unwrap();
        std::fs::write(dir.join("ui/index.html"), b"<p>").unwrap();
        dir
    }

    fn sign(dir: &Path, seed: u8) -> PublisherKey {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let signature = key.sign(&content_digest(dir).unwrap());
        std::fs::write(dir.join(SIGNATURE_FILE), hex::encode(signature.to_bytes())).unwrap();
        PublisherKey(key.verifying_key())
    }

    fn store(keys: &[PublisherKey]) -> TrustStore {
        TrustStore {
            keys: keys.to_vec(),
        }
    }

    #[test]
    fn the_committed_demo_signatures_verify_with_the_demo_key() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let key = PublisherKey::from_file(&root.join("examples/keys/weft-sign.pub")).unwrap();
        for demo in ["org.weft.demo.counter", "org.weft.demo.notes"] {
            let package = root.join("examples").join(demo);
            assert_eq!(store(&[key]).signer(&package).unwrap(), Some(key), "{demo}");
        }
    }

    #[test]
    fn any_changed_file_breaks_the_signature() {
        let dir = dir("tamper");
        let key = sign(&dir, 1);
        assert_eq!(store(&[key]).signer(&dir).unwrap(), Some(key));
        std::fs::write(dir.join("ui/index.html"), b"<p>changed").unwrap();
        assert_eq!(store(&[key]).signer(&dir).unwrap(), None);
        std::fs::write(dir.join("ui/index.html"), b"<p>").unwrap();
        std::fs::write(dir.join("ui/extra.js"), b"").unwrap();
        assert_eq!(store(&[key]).signer(&dir).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_trusted_keys_are_signers_and_unsigned_packages_have_none() {
        let dir = dir("untrusted");
        assert_eq!(store(&[]).signer(&dir).unwrap(), None);
        let key = sign(&dir, 2);
        let other = PublisherKey(SigningKey::from_bytes(&[3; 32]).verifying_key());
        assert_eq!(store(&[other]).signer(&dir).unwrap(), None);
        assert_eq!(store(&[other, key]).signer(&dir).unwrap(), Some(key));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn names_cannot_imitate_other_inventory_lines() {
        // With files ui/policy.txt and ui/z.css signed, a directory named
        // "ui/policy.txt\t<hash>\nui" holding z.css would reproduce the
        // inventory without policy.txt. Such names have no digest.
        let dir = dir("forged");
        std::fs::write(dir.join("ui/policy.txt"), b"default-src 'self'").unwrap();
        std::fs::write(dir.join("ui/z.css"), b"p{}").unwrap();
        let signed = content_digest(&dir).unwrap();
        let policy_hash = hex::encode(Sha256::digest(b"default-src 'self'"));
        std::fs::remove_file(dir.join("ui/policy.txt")).unwrap();
        let forged = dir.join(format!("ui/policy.txt\t{policy_hash}\nui"));
        std::fs::create_dir_all(&forged).unwrap();
        std::fs::rename(dir.join("ui/z.css"), forged.join("z.css")).unwrap();
        let refused = content_digest(&dir);
        assert!(matches!(refused, Err(TrustError::ControlCharacter(_))));
        assert_ne!(refused.ok(), Some(signed));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn owner_locks_exclude_each_other() {
        let home = std::env::temp_dir().join(format!("weft_trust_lock_{}", std::process::id()));
        let held = lock_owner(&home, "org.weft.test.lock").unwrap();
        let path = home.join("weft/owners/.org.weft.test.lock.lock");
        let other = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert!(other.try_lock().is_err());
        drop(held);
        assert!(other.try_lock().is_ok());
        assert!(lock_owner(&home, "../outside").is_err());
        assert!(!home.join("weft/outside.lock").exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn links_and_special_files_have_no_content_digest() {
        let dir = dir("links");
        std::os::unix::fs::symlink("/etc/hostname", dir.join("ui/linked")).unwrap();
        assert!(matches!(content_digest(&dir), Err(TrustError::Link(_))));
        std::fs::remove_file(dir.join("ui/linked")).unwrap();
        std::os::unix::fs::symlink(dir.join("ui"), dir.join("view")).unwrap();
        assert!(matches!(content_digest(&dir), Err(TrustError::Link(_))));
        std::fs::remove_file(dir.join("view")).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            dir.join("fifo"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        assert!(matches!(content_digest(&dir), Err(TrustError::NotAFile(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn trust_stores_load_pub_files_and_refuse_malformed_keys() {
        let dir = dir("store");
        let keys = dir.join("keys");
        std::fs::create_dir_all(&keys).unwrap();
        let key = PublisherKey(SigningKey::from_bytes(&[4; 32]).verifying_key());
        std::fs::write(keys.join("a.pub"), key.to_hex()).unwrap();
        std::fs::write(keys.join("notes.txt"), "not a key").unwrap();
        let loaded = TrustStore::load(&[keys.clone(), dir.join("missing")]).unwrap();
        assert_eq!(loaded.keys(), [key]);
        std::fs::write(keys.join("b.pub"), "zz").unwrap();
        assert!(matches!(
            TrustStore::load(&[keys]),
            Err(TrustError::Malformed(..))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn owner_records_round_trip_and_are_never_replaced() {
        let dir = dir("owners");
        let key = PublisherKey(SigningKey::from_bytes(&[5; 32]).verifying_key());
        let verified = owner_record_path(&dir, "org.weft.test.a");
        let development = owner_record_path(&dir, "org.weft.test.b");
        assert_eq!(read_owner(&verified).unwrap(), None);
        write_owner(&verified, Owner::Verified(key)).unwrap();
        write_owner(&development, Owner::Development).unwrap();
        assert_eq!(read_owner(&verified).unwrap(), Some(Owner::Verified(key)));
        assert_eq!(read_owner(&development).unwrap(), Some(Owner::Development));
        assert!(write_owner(&verified, Owner::Development).is_err());
        assert_eq!(read_owner(&verified).unwrap(), Some(Owner::Verified(key)));
        std::fs::write(&development, "someone else\n").unwrap();
        assert!(read_owner(&development).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
