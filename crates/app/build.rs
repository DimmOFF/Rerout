// Build script of rerout.exe:
//  * exposes the build date and the git commit to the program (REROUT_BUILD_DATE, REROUT_GIT),
//  * generates the Windows resource file (icon + version info) from the crate version,
//    so the version is written in exactly one place: [workspace.package] in the root Cargo.toml.
// Embedding needs rc.exe from the Windows SDK (part of the MSVC Build Tools); if it cannot be
// found the build still succeeds, just without icon and version info.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Days since 1970-01-01 -> (year, month, day), UTC (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=rerout.ico");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    if std::path::Path::new("../../.git/HEAD").exists() {
        println!("cargo:rerun-if-changed=../../.git/HEAD");
    }

    // ---- build date and commit ----
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil((secs / 86_400) as i64);
    println!("cargo:rustc-env=REROUT_BUILD_DATE={y:04}-{m:02}-{d:02}");
    println!(
        "cargo:rustc-env=REROUT_GIT={}",
        git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default()
    );

    // ---- Windows resources: icon + version info ----
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    // "0.2.0-beta.1" -> 0,2,0
    let nums: Vec<u32> = ver
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(3)
        .map(|s| s.parse().unwrap_or(0))
        .collect();
    let part = |i: usize| nums.get(i).copied().unwrap_or(0);
    let (major, minor, patch) = (part(0), part(1), part(2));

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let icon = manifest.join("rerout.ico").to_string_lossy().replace('\\', "\\\\");

    let rc = format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
 FILEVERSION {major},{minor},{patch},0
 PRODUCTVERSION {major},{minor},{patch},0
 FILEOS 0x4L
 FILETYPE 0x1L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "FileDescription", "Rerout - per-application network interface control"
            VALUE "ProductName", "Rerout"
            VALUE "FileVersion", "{ver}"
            VALUE "ProductVersion", "{ver}"
            VALUE "InternalName", "rerout"
            VALUE "OriginalFilename", "rerout.exe"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    );
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    let rc_path = out_dir.join("rerout.rc");
    if std::fs::write(&rc_path, rc).is_ok() {
        let _ = embed_resource::compile(&rc_path, embed_resource::NONE);
    }
}
