use std::path::PathBuf;
use std::process::Command as StdCommand;

#[cfg(test)]
use super::binary::{SAFE_PATH_DIRS, canonicalize_executable, resolve_nb_override};
#[cfg(test)]
use std::path::Path;

/// Login home for the current user (ignores env `HOME`, which tests
/// overwrite with fixture tempdirs).
///
/// Order (Unix):
/// 1. macOS `id -P` passwd-style line (Open Directory–backed accounts)
/// 2. macOS `dscl` `NFSHomeDirectory` for the current user name
/// 3. `/etc/passwd` by UID (Linux and legacy macOS local files)
///
/// External tools are invoked by absolute path so a poisoned process
/// `PATH` cannot block discovery.
pub(super) fn login_home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        if let Some(home) = home_from_id_p() {
            return Some(home);
        }
        if let Some(home) = home_from_dscl() {
            return Some(home);
        }
        home_from_etc_passwd_uid()
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }
}

/// macOS `id -P`: print the current user as a passwd-style entry
/// (works for Open Directory accounts that are absent from
/// `/etc/passwd`). GNU/Linux `id` typically rejects `-P`.
#[cfg(unix)]
fn home_from_id_p() -> Option<PathBuf> {
    for id_bin in ["/usr/bin/id", "/bin/id"] {
        let output = match StdCommand::new(id_bin).arg("-P").output() {
            Ok(o) => o,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.lines().next()?;
        if let Some(home) = home_from_passwd_style_line(line) {
            return Some(home);
        }
    }
    None
}

/// macOS Directory Service home lookup for the current user name.
#[cfg(unix)]
fn home_from_dscl() -> Option<PathBuf> {
    let user = current_username()?;
    // Only meaningful on macOS; spawn failure is non-fatal.
    let output = match StdCommand::new("/usr/bin/dscl")
        .args([".", "-read", &format!("/Users/{user}"), "NFSHomeDirectory"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return None,
    };
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_dscl_home(&stdout)
}

#[cfg(unix)]
fn home_from_etc_passwd_uid() -> Option<PathBuf> {
    let uid = current_uid()?;
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let mut fields = line.split(':');
        let _name = fields.next()?;
        let _passwd = fields.next()?;
        let file_uid = fields.next()?.parse::<u32>().ok()?;
        if file_uid != uid {
            continue;
        }
        let _gid = fields.next()?;
        let _gecos = fields.next()?;
        let home = fields.next()?;
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    None
}

/// Home directory from a passwd-style colon line.
///
/// Traditional (7 fields): `name:pw:uid:gid:gecos:home:shell` → home @ 5.
/// macOS `id -P` (10 fields):
/// `name:pw:uid:gid:class:change:expire:gecos:home:shell` → home @ 8.
#[cfg_attr(not(unix), allow(dead_code))]
fn home_from_passwd_style_line(line: &str) -> Option<PathBuf> {
    let fields: Vec<&str> = line.trim().split(':').collect();
    let home = if fields.len() >= 10 {
        fields[8]
    } else if fields.len() >= 7 {
        fields[5]
    } else {
        return None;
    };
    if home.is_empty() {
        None
    } else {
        Some(PathBuf::from(home))
    }
}

/// Parse `dscl … NFSHomeDirectory` stdout.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_dscl_home(stdout: &str) -> Option<PathBuf> {
    for line in stdout.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("NFSHomeDirectory:")
            .or_else(|| line.strip_prefix("dsAttrTypeNative:NFSHomeDirectory:"))
        else {
            continue;
        };
        let home = rest.trim();
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    None
}

#[cfg(unix)]
fn current_username() -> Option<String> {
    for id_bin in ["/usr/bin/id", "/bin/id"] {
        let output = match StdCommand::new(id_bin).arg("-un").output() {
            Ok(o) => o,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

#[cfg(unix)]
fn current_uid() -> Option<u32> {
    // Absolute id(1) — does not depend on process PATH (may be poisoned).
    for id_bin in ["/usr/bin/id", "/bin/id"] {
        let output = match StdCommand::new(id_bin).arg("-u").output() {
            Ok(o) => o,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Ok(uid) = stdout.trim().parse::<u32>() {
            return Some(uid);
        }
    }
    // Linux-only fallback when id(1) is unavailable.
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("Uid:") else {
            continue;
        };
        return rest.split_whitespace().next()?.parse().ok();
    }
    None
}

#[cfg(test)]
mod nb_resolve_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn relative_nb_override_is_rejected() {
        let err = resolve_nb_override(Path::new("relative/nb")).unwrap_err();
        assert!(
            err.contains("absolute"),
            "expected absolute-path error, got {err}"
        );
    }

    #[test]
    fn relative_candidate_is_not_canonicalized() {
        assert!(canonicalize_executable(Path::new("nb")).is_none());
        assert!(canonicalize_executable(Path::new("./nb")).is_none());
    }

    #[test]
    fn safe_path_dirs_include_homebrew_prefix() {
        assert!(
            SAFE_PATH_DIRS.contains(&"/opt/homebrew/bin"),
            "Apple Silicon Homebrew must be in SAFE_PATH_DIRS for poisoned-PATH discovery"
        );
    }

    #[test]
    fn parses_macos_id_p_home_field() {
        let line = "me:********:501:20::0:0:Me:/Users/me:/bin/zsh";
        assert_eq!(
            home_from_passwd_style_line(line),
            Some(PathBuf::from("/Users/me"))
        );
    }

    #[test]
    fn parses_traditional_passwd_home_field() {
        let line = "me:x:1000:1000:Me:/home/me:/bin/bash";
        assert_eq!(
            home_from_passwd_style_line(line),
            Some(PathBuf::from("/home/me"))
        );
    }

    #[test]
    fn parses_dscl_nfs_home_directory() {
        assert_eq!(
            parse_dscl_home("NFSHomeDirectory: /Users/me\n"),
            Some(PathBuf::from("/Users/me"))
        );
        assert_eq!(
            parse_dscl_home("dsAttrTypeNative:NFSHomeDirectory: /Users/me\n"),
            Some(PathBuf::from("/Users/me"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn absolute_executable_is_canonicalized() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("fake-nb");
        fs::write(&bin, b"#!/bin/sh\n").expect("write");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
        let abs = bin.canonicalize().expect("canonicalize fixture path");
        let got = canonicalize_executable(&abs).expect("accept absolute executable");
        assert!(got.is_absolute());
        assert_eq!(got, abs);
    }
}
