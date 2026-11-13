//! "Add from running applications" view: processes grouped by executable,
//! with their network activity and the interfaces they currently use.

use crate::{conns, icons::IconCache, theme, watcher};
use eframe::egui::{self, RichText};
use rerout_common::adapters::Adapter;
use rerout_common::norm_path;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

pub enum Action {
    None,
    Back,
    Add(String),
}

struct AppEntry {
    name: String,
    path: String,
    pids: Vec<u32>,
    /// established TCP connections
    tcp: usize,
    listen: usize,
    udp: usize,
    /// names of interfaces the app's sockets are bound to
    ifaces: Vec<String>,
    /// a few remote endpoints
    remotes: Vec<String>,
    remote_set: HashSet<String>,
}

pub struct Picker {
    search: String,
    only_net: bool,
    hide_system: bool,
    entries: Vec<AppEntry>,
    last: Instant,
    /// pid -> exe path (cached, looking it up is relatively expensive)
    paths: HashMap<u32, Option<String>>,
}

fn windows_dir() -> String {
    let d = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    format!("{}\\", d.to_lowercase())
}

impl Picker {
    pub fn new(adapters: &[Adapter]) -> Self {
        let mut p = Self {
            search: String::new(),
            only_net: true,
            hide_system: true,
            entries: Vec::new(),
            last: Instant::now(),
            paths: HashMap::new(),
        };
        p.refresh(adapters);
        p
    }

    pub fn refresh(&mut self, adapters: &[Adapter]) {
        self.last = Instant::now();
        let procs = watcher::processes();
        let alive: HashSet<u32> = procs.iter().map(|(p, _, _)| *p).collect();
        self.paths.retain(|p, _| alive.contains(p));
        let me = std::env::current_exe()
            .ok()
            .map(|p| norm_path(&p.to_string_lossy()));

        let mut by_path: HashMap<String, AppEntry> = HashMap::new();
        let mut pid_key: HashMap<u32, String> = HashMap::new();
        for (pid, _, _) in &procs {
            if *pid == 0 || *pid == 4 {
                continue;
            }
            let path = self
                .paths
                .entry(*pid)
                .or_insert_with(|| watcher::full_path(*pid))
                .clone();
            let Some(path) = path else { continue };
            let key = norm_path(&path);
            if Some(&key) == me.as_ref() {
                continue; // not Rerout itself
            }
            let name = Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let entry = by_path.entry(key.clone()).or_insert_with(|| AppEntry {
                name,
                path: path.clone(),
                pids: Vec::new(),
                tcp: 0,
                listen: 0,
                udp: 0,
                ifaces: Vec::new(),
                remotes: Vec::new(),
                remote_set: HashSet::new(),
            });
            entry.pids.push(*pid);
            pid_key.insert(*pid, key);
        }

        for c in conns::raw_all() {
            let Some(key) = pid_key.get(&c.pid) else { continue };
            let Some(e) = by_path.get_mut(key) else { continue };

            if c.proto.starts_with("TCP") {
                match c.state {
                    "ESTABLISHED" => e.tcp += 1,
                    "LISTEN" => {
                        e.listen += 1;
                        continue;
                    }
                    _ => continue,
                }
            } else {
                e.udp += 1;
            }

            if !c.local.is_unspecified() && !c.local.is_loopback() {
                if let Some(a) = conns::adapter_for(c.local, adapters) {
                    if !e.ifaces.contains(&a.name) {
                        e.ifaces.push(a.name.clone());
                    }
                }
            }
            if let Some((ip, port)) = c.remote {
                if !ip.is_loopback() && !ip.is_unspecified() {
                    let s = conns::fmt_addr(ip, port);
                    if e.remote_set.insert(s.clone()) && e.remotes.len() < 2 {
                        e.remotes.push(s);
                    }
                }
            }
        }

        let mut entries: Vec<AppEntry> = by_path.into_values().collect();
        entries.sort_by(|a, b| {
            let wa = a.tcp + a.udp + a.listen;
            let wb = b.tcp + b.udp + b.listen;
            wb.cmp(&wa)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.entries = entries;
    }

    /// `existing`: normalized paths that are already in the list.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        adapters: &[Adapter],
        existing: &HashSet<String>,
        icons: &mut IconCache,
    ) -> Action {
        if self.last.elapsed() > Duration::from_secs(2) {
            self.refresh(adapters);
        }
        ui.ctx().request_repaint_after(Duration::from_secs(2));

        let mut action = Action::None;
        ui.horizontal(|ui| {
            if ui.button("< Back").clicked() {
                action = Action::Back;
            }
            ui.heading("Add from running applications");
        });

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search by name or path...")
                    .desired_width(280.0),
            );
            ui.checkbox(&mut self.only_net, "Only apps with network activity");
            ui.checkbox(&mut self.hide_system, "Hide Windows system apps");
            if ui.button("Refresh").clicked() {
                self.refresh(adapters);
            }
        });

        let q = self.search.to_lowercase();
        let windir = windows_dir();
        let visible: Vec<&AppEntry> = self
            .entries
            .iter()
            .filter(|e| {
                let lower = e.path.to_lowercase();
                if self.hide_system && lower.starts_with(&windir) {
                    return false;
                }
                if self.only_net && e.tcp + e.udp + e.listen == 0 {
                    return false;
                }
                q.is_empty() || e.name.to_lowercase().contains(&q) || lower.contains(&q)
            })
            .collect();
        ui.label(RichText::new(format!("{} applications", visible.len())).small().weak());

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, e) in visible.iter().enumerate() {
                ui.push_id(i, |ui| {
                    let already = existing.contains(&norm_path(&e.path));
                    theme::card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            icons.show(ui, &e.path, 32.0);
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&e.name).strong().size(15.0));
                                    if e.pids.len() > 1 {
                                        ui.label(
                                            RichText::new(format!("{} processes", e.pids.len()))
                                                .small()
                                                .weak(),
                                        );
                                    }
                                });
                                ui.label(RichText::new(&e.path).small().weak());
                                ui.horizontal_wrapped(|ui| {
                                    if e.tcp + e.udp + e.listen == 0 {
                                        ui.label(RichText::new("No network activity").small().weak());
                                    }
                                    if e.tcp > 0 {
                                        theme::chip(ui, format!("TCP {}", e.tcp), theme::ACCENT);
                                    }
                                    if e.udp > 0 {
                                        theme::chip(ui, format!("UDP {}", e.udp), theme::ACCENT);
                                    }
                                    if e.listen > 0 {
                                        theme::chip(ui, format!("Listening {}", e.listen), theme::GRAY);
                                    }
                                    for n in &e.ifaces {
                                        theme::chip(ui, format!("via {n}"), theme::GREEN);
                                    }
                                });
                                if !e.remotes.is_empty() {
                                    let more = e.remote_set.len().saturating_sub(e.remotes.len());
                                    let mut text = format!("Connected to {}", e.remotes.join(", "));
                                    if more > 0 {
                                        text.push_str(&format!(" and {more} more"));
                                    }
                                    ui.label(RichText::new(text).small().weak());
                                }
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if already {
                                    ui.label(RichText::new("Already added").weak());
                                } else if ui.add(theme::primary("Add")).clicked() {
                                    action = Action::Add(e.path.clone());
                                }
                            });
                        });
                    });
                });
            }
        });
        action
    }
}
