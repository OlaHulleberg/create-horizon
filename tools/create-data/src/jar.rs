//! Create's jar, fetched for the player from Create's official distribution. The Create team
//! publishes each release on Modrinth (project `LNytGWDc`); this pins one exact file there by URL
//! and hashes, the same file `create/horizon-mod.toml` names in its `[[jar]]` (which the launcher
//! layers, assets and data, under the mod). The jar's assets are All Rights Reserved: it is
//! downloaded into the player's own cache and read there, never committed or shipped.

use sha2::{Digest, Sha512};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// One exact file of a Modrinth project version.
pub struct Pinned {
    pub version: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub sha512: &'static str,
}

/// Create 6.0.10 for 1.21.1 (NeoForge), Modrinth version `UjX6dr61`: the newest release, whose
/// assets are those of 6.0.11's sources but for two textures. (Modrinth's SHA-1 of it:
/// `0e97e49837bed766e6f28a4c95b04885d6acc353`.)
pub const CREATE: Pinned = Pinned {
    version: "6.0.10",
    file: "create-1.21.1-6.0.10.jar",
    url: "https://cdn.modrinth.com/data/LNytGWDc/versions/UjX6dr61/create-1.21.1-6.0.10.jar",
    sha512: "11cc8fc049d2f67f6548c7abfada6b82a3adb5c7ca410a742de04bbca76e03862c518721b88d806f6e6d768a4d68531fdb903a85859b25d1484d550cc7bafd4b",
};

/// Where downloaded jars are kept: `$CREATE_DATA_CACHE`, else the user's cache folder
/// (`$XDG_CACHE_HOME`, `~/.cache`, `%LOCALAPPDATA%`) under `create-horizon/`.
pub fn cache_dir() -> Result<PathBuf, String> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var("CREATE_DATA_CACHE") {
        return Ok(dir);
    }
    let base = var("XDG_CACHE_HOME")
        .or_else(|| var("LOCALAPPDATA"))
        .or_else(|| var("HOME").map(|h| h.join(".cache")))
        .ok_or("no cache folder: set CREATE_DATA_CACHE")?;
    Ok(base.join("create-horizon"))
}

/// The lowercase hexadecimal SHA-512 of everything `reader` yields, copied into `writer`.
fn sha512_copy(mut reader: impl Read, mut writer: impl Write) -> io::Result<String> {
    let mut hash = Sha512::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hash.update(&buf[..n]);
        writer.write_all(&buf[..n])?;
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The SHA-512 of a file.
pub fn sha512_of(path: &Path) -> io::Result<String> {
    sha512_copy(fs::File::open(path)?, io::sink())
}

/// The pinned file in `dir`, downloaded with `download` (url, writer) unless a copy with the
/// pinned SHA-512 is already there. A download is checked before it takes the file's name; a
/// cached copy with another hash is replaced.
pub fn cached(
    dir: &Path,
    pin: &Pinned,
    download: impl FnOnce(&str, &mut dyn Write) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let path = dir.join(pin.file);
    match sha512_of(&path) {
        Ok(hash) if hash == pin.sha512 => return Ok(path),
        Ok(_) => eprintln!(
            "{}: SHA-512 differs from the pin; downloading again",
            path.display()
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part = dir.join(format!("{}.part", pin.file));
    let result = (|| {
        let mut file = io::BufWriter::new(
            fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?,
        );
        let mut hashing = HashingWriter {
            inner: &mut file,
            hash: Sha512::new(),
        };
        download(pin.url, &mut hashing)?;
        let hash: String = hashing
            .hash
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        file.flush()
            .map_err(|e| format!("{}: {e}", part.display()))?;
        if hash != pin.sha512 {
            return Err(format!(
                "{}: SHA-512 mismatch: expected {}, got {hash}",
                pin.url, pin.sha512
            ));
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

struct HashingWriter<'a, W: Write> {
    inner: &'a mut W,
    hash: Sha512,
}

impl<W: Write> Write for HashingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// The pinned jar from the user's cache, downloaded from Modrinth's CDN the first time.
pub fn fetch(pin: &Pinned) -> Result<PathBuf, String> {
    let dir = cache_dir()?;
    cached(&dir, pin, |url, out| {
        eprintln!("downloading {url}");
        let agent = ureq::AgentBuilder::new()
            .user_agent(concat!("create-data/", env!("CARGO_PKG_VERSION")))
            .build();
        let response = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
        io::copy(&mut response.into_reader(), out).map_err(|e| format!("{url}: {e}"))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str, len: usize) -> bool {
        s.len() == len
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    #[test]
    fn the_pin_is_well_formed_and_matches_the_manifest() {
        assert!(hex(CREATE.sha512, 128));
        assert!(
            CREATE
                .url
                .starts_with("https://cdn.modrinth.com/data/LNytGWDc/versions/")
        );
        assert!(CREATE.url.ends_with(&format!("/{}", CREATE.file)));
        assert!(CREATE.file.contains(CREATE.version));
        // The launcher layers the file the manifest names: it must be the one prepare reads.
        let manifest = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../create/horizon-mod.toml"),
        )
        .unwrap();
        assert!(manifest.contains(&format!("sha512 = \"{}\"", CREATE.sha512)));
        assert!(manifest.contains(&format!("version = \"{}\"", CREATE.version)));
    }

    fn sha512(bytes: &[u8]) -> String {
        sha512_copy(bytes, io::sink()).unwrap()
    }

    #[test]
    fn caches_verifies_and_replaces() {
        let dir = std::env::temp_dir().join(format!("create-data-jar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let body = b"not really a jar".to_vec();
        let hash = sha512(&body);
        let pin = Pinned {
            version: "1",
            file: "fake-1.jar",
            url: "https://example.invalid/fake-1.jar",
            sha512: Box::leak(hash.into_boxed_str()),
        };
        let serve = |bytes: Vec<u8>| {
            move |url: &str, out: &mut dyn Write| {
                assert_eq!(url, "https://example.invalid/fake-1.jar");
                out.write_all(&bytes).map_err(|e| e.to_string())
            }
        };
        // A download with the wrong bytes is refused and leaves nothing behind.
        let err = cached(&dir, &pin, serve(b"tampered".to_vec())).unwrap_err();
        assert!(err.contains("SHA-512 mismatch"), "{err}");
        assert!(!dir.join("fake-1.jar").exists());
        assert!(!dir.join("fake-1.jar.part").exists());
        // The right bytes are kept under the file's name.
        let path = cached(&dir, &pin, serve(body.clone())).unwrap();
        assert_eq!(fs::read(&path).unwrap(), body);
        // A cached copy is used without downloading.
        let again = cached(
            &dir,
            &pin,
            |_: &str, _: &mut dyn Write| -> Result<(), String> {
                panic!("downloaded a cached jar")
            },
        )
        .unwrap();
        assert_eq!(again, path);
        // A corrupted cache entry is downloaded again.
        fs::write(&path, b"corrupt").unwrap();
        cached(&dir, &pin, serve(body.clone())).unwrap();
        assert_eq!(fs::read(&path).unwrap(), body);
        assert_eq!(sha512_of(&path).unwrap(), pin.sha512);
        fs::remove_dir_all(&dir).unwrap();
    }
}
