//! Program version. The single source of truth is `[workspace.package] version`
//! in the root Cargo.toml; build.rs adds the build date and the git commit.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_DATE: &str = env!("REROUT_BUILD_DATE");
/// short commit hash, empty when built outside a git repository
pub const GIT: &str = env!("REROUT_GIT");

/// Text for the "about" tooltip.
pub fn about() -> String {
    let mut s = format!("Rerout {VERSION}\nBuilt: {BUILD_DATE}");
    if !GIT.is_empty() {
        s.push_str(&format!("\nCommit: {GIT}"));
    }
    s.push_str(&format!(
        "\nSettings: {}",
        rerout_common::config::config_path().display()
    ));
    s
}
