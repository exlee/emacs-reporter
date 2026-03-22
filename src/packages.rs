use std::path::{Path, PathBuf};

pub struct PackageEntry {
    pub id: String,
    pub name: String,
    pub version: String,
}

/// Collect installed ELPA packages for the given Emacs binary.
/// Returns an empty vec if the elpa directory cannot be found.
pub fn collect_packages(binary_path: &Path) -> Vec<PackageEntry> {
    let elpa_dir = match find_elpa_dir(binary_path) {
        Some(d) => d,
        None => return vec![],
    };

    let rd = match std::fs::read_dir(&elpa_dir) {
        Ok(rd) => rd,
        Err(_) => return vec![],
    };

    let mut packages = Vec::new();
    for entry in rd.flatten() {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !ft.is_dir() {
            continue;
        }
        let raw = entry.file_name();
        let s = raw.to_string_lossy();
        if let Some((name, version)) = parse_name_version(&s) {
            packages.push(PackageEntry {
                id: stable_id(name, version),
                name: name.to_owned(),
                version: version.to_owned(),
            });
        }
    }
    packages
}

// ── elpa path discovery ───────────────────────────────────────────────────────

fn find_elpa_dir(binary_path: &Path) -> Option<PathBuf> {
    let home = home_dir(binary_path)?;

    let candidates = [
        home.join(".emacs.d").join("elpa"),
        home.join(".config").join("emacs").join("elpa"),
    ];
    for path in &candidates {
        if path.is_dir() {
            return Some(path.clone());
        }
    }

    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg).join("emacs").join("elpa");
        if p.is_dir() {
            return Some(p);
        }
    }

    None
}

fn home_dir(_binary_path: &Path) -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home);
        if p.is_dir() {
            return Some(p);
        }
    }
    home_via_getpwuid()
}

fn home_via_getpwuid() -> Option<PathBuf> {
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        let dir = (*pw).pw_dir;
        if dir.is_null() {
            return None;
        }
        let s = std::ffi::CStr::from_ptr(dir).to_string_lossy().into_owned();
        let p = PathBuf::from(s);
        if p.is_dir() {
            Some(p)
        } else {
            None
        }
    }
}

// ── directory name parsing ────────────────────────────────────────────────────

/// Parse `"magit-20240101.123"` into `("magit", "20240101.123")`.
/// Scans right-to-left for `-` immediately followed by an ASCII digit.
fn parse_name_version(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    for i in (1..bytes.len()).rev() {
        if bytes[i - 1] == b'-' && bytes[i].is_ascii_digit() {
            return Some((&s[..i - 1], &s[i..]));
        }
    }
    None
}

// ── stable ID generation ──────────────────────────────────────────────────────

/// UUID v5 (SHA-1 name hash) of `"<name>-<version>"`.
/// Produces the same ID for the same package across different databases.
fn stable_id(name: &str, version: &str) -> String {
    let key = format!("{name}-{version}");
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, key.as_bytes()).to_string()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_typical() {
        assert_eq!(
            parse_name_version("magit-20240101.123"),
            Some(("magit", "20240101.123"))
        );
        assert_eq!(
            parse_name_version("use-package-2.4.1"),
            Some(("use-package", "2.4.1"))
        );
    }

    #[test]
    fn parse_skip_non_package() {
        assert!(parse_name_version("archives").is_none());
        assert!(parse_name_version("gnupg").is_none());
    }

    #[test]
    fn stable_id_is_deterministic() {
        assert_eq!(
            stable_id("magit", "20240101.123"),
            stable_id("magit", "20240101.123")
        );
    }
}
