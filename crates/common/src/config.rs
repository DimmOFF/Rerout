use crate::adapters::Adapter;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Rule for one application.
///
/// Interface model: `allowed` is an ordered list of adapter GUIDs (top = highest
/// priority). Every other interface is denied. Traffic goes through the first
/// allowed interface that is up; if there is none, network access is blocked.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Rule {
    pub exe_path: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// false until the user has configured interfaces: such a rule does nothing
    #[serde(default)]
    pub configured: bool,
    #[serde(default, alias = "adapter_guids")]
    pub allowed: Vec<String>,
    #[serde(default = "default_true")]
    pub allow_loopback: bool,
    /// Last known friendly names of allowed adapters (GUID -> name), so an
    /// interface that is gone right now can still be shown by name
    #[serde(default)]
    pub known_names: HashMap<String, String>,
}

impl Rule {
    pub fn new(exe_path: String) -> Self {
        Self {
            exe_path,
            enabled: true,
            configured: false,
            allowed: Vec::new(),
            allow_loopback: true,
            known_names: HashMap::new(),
        }
    }

    pub fn is_allowed(&self, guid: &str) -> bool {
        self.allowed.iter().any(|g| g.eq_ignore_ascii_case(guid))
    }
}

fn default_true() -> bool {
    true
}

/// Version of the config file format. Bump it when the format changes and add a
/// migration step in `load()`.
pub const CONFIG_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    /// format version of the file (0 = written before versioning existed)
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

fn appdata() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn config_path() -> PathBuf {
    appdata().join("rerout").join("config.json")
}

/// Settings folder of versions before the rename (FBind).
fn legacy_config_path() -> PathBuf {
    appdata().join("fbind").join("config.json")
}

pub fn load() -> Config {
    let read = |p: PathBuf| {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str::<Config>(&s).ok())
    };
    let mut cfg = read(config_path())
        .or_else(|| read(legacy_config_path()))
        .unwrap_or_default();
    // ---- migrations ----
    if cfg.version < 1 {
        // v0 -> v1: rules created before "configured" existed already had interfaces chosen
        for r in &mut cfg.rules {
            if !r.configured && !r.allowed.is_empty() {
                r.configured = true;
            }
        }
    }
    // never lower the version of a file written by a newer Rerout
    cfg.version = cfg.version.max(CONFIG_VERSION);
    cfg
}

/// Atomic write: temp file, then rename.
pub fn save(cfg: &Config) -> std::io::Result<()> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = cfg.clone();
    out.version = out.version.max(CONFIG_VERSION);
    let json = serde_json::to_string_pretty(&out).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

/// Normalize a path for comparison (case, slashes, \\?\ prefix).
pub fn norm_path(s: &str) -> String {
    s.trim()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase()
}

/// First allowed adapter that is up (priority = order in `allowed`).
pub fn resolve<'a>(rule: &Rule, adapters: &'a [Adapter]) -> Option<&'a Adapter> {
    rule.allowed.iter().find_map(|g| {
        adapters
            .iter()
            .find(|a| a.up && a.guid.eq_ignore_ascii_case(g))
    })
}
