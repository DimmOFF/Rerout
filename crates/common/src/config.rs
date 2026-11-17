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
    /// id of the group this app is shown in (0 = no group)
    #[serde(default)]
    pub group: u32,
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
            group: 0,
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
pub const CONFIG_VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    /// format version of the file (0 = written before versioning existed)
    #[serde(default)]
    pub version: u32,
    /// Display order of the applications: the order of `rules` (within a group and among ungrouped ones).
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Groups, in display order
    #[serde(default)]
    pub groups: Vec<Group>,
    /// list view mode: false = expanded, true = compact
    #[serde(default)]
    pub compact: bool,
}

/// A named, collapsible group of applications in the main list.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Group {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
}

/// Where a dragged application goes.
#[derive(Clone, Copy, Debug)]
pub enum Dest {
    /// right before / after another application (takes over its group)
    Before(usize),
    After(usize),
    /// to the end of a group (0 = to the end of the ungrouped ones)
    EndOfGroup(u32),
}

impl Config {
    pub fn new_group(&mut self, name: &str) -> u32 {
        let id = self.groups.iter().map(|g| g.id).max().unwrap_or(0) + 1;
        self.groups.push(Group { id, name: name.to_string(), collapsed: false });
        id
    }

    /// Delete a group; its applications stay in the list, without a group.
    pub fn delete_group(&mut self, id: u32) {
        self.groups.retain(|g| g.id != id);
        for r in &mut self.rules {
            if r.group == id {
                r.group = 0;
            }
        }
    }

    pub fn move_rule(&mut self, from: usize, dest: Dest) {
        if from >= self.rules.len() {
            return;
        }
        let (group, mut at) = match dest {
            Dest::Before(t) if t < self.rules.len() => (self.rules[t].group, t),
            Dest::After(t) if t < self.rules.len() => (self.rules[t].group, t + 1),
            Dest::EndOfGroup(g) => (g, self.rules.len()),
            _ => return,
        };
        let mut rule = self.rules.remove(from);
        if from < at {
            at -= 1;
        }
        rule.group = group;
        let at = at.min(self.rules.len());
        self.rules.insert(at, rule);
    }

    /// Move group `id` to the end of the group list.
    pub fn move_group_to_end(&mut self, id: u32) {
        if let Some(from) = self.groups.iter().position(|g| g.id == id) {
            let group = self.groups.remove(from);
            self.groups.push(group);
        }
    }

    /// Move group `id` right before / after group `target`.
    pub fn move_group(&mut self, id: u32, target: u32, after: bool) {
        if id == target {
            return;
        }
        let Some(from) = self.groups.iter().position(|g| g.id == id) else { return };
        let group = self.groups.remove(from);
        match self.groups.iter().position(|g| g.id == target) {
            Some(mut at) => {
                if after {
                    at += 1;
                }
                self.groups.insert(at, group);
            }
            None => self.groups.insert(from.min(self.groups.len()), group),
        }
    }
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
