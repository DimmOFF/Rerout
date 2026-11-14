//! "Configure interfaces" view: two columns (Allowed / Denied) with drag & drop.
//! Order inside Allowed = priority. Everything not in Allowed is denied.
//!
//! Changes are applied to the rule immediately. "Revert all changes" restores the
//! settings the rule had when the view was opened.

use crate::{icons::IconCache, theme};
use eframe::egui::{self, Align, Color32, Layout, RichText, Stroke};
use rerout_common::adapters::Adapter;
use rerout_common::config::Rule;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, PartialEq)]
struct Loc {
    col: usize,
    row: usize,
}

pub enum Action {
    None,
    /// something changed: apply the current state to the rule right now
    Changed,
    /// restore the state from when the view was opened
    Revert,
    /// leave the view (changes are already applied)
    Close,
}

pub struct Configure {
    pub exe_path: String,
    /// [0] = Allowed (priority order), [1] = Denied; adapter GUIDs
    cols: [Vec<String>; 2],
    allow_loopback: bool,
    /// last known names of adapters that may be missing right now
    names: HashMap<String, String>,
    /// the rule as it was when the view was opened (for Revert)
    original: Rule,
    /// anything changed since the view was opened
    touched: bool,
    /// false for a brand-new rule until the first change: such a rule does nothing yet
    activated: bool,
}

impl Configure {
    pub fn new(rule: &Rule, adapters: &[Adapter]) -> Self {
        let allowed: Vec<String> = if rule.configured {
            rule.allowed.clone()
        } else {
            // New rule: start with every interface that is up, the user removes the unwanted ones
            adapters.iter().filter(|a| a.up).map(|a| a.guid.clone()).collect()
        };
        let denied: Vec<String> = adapters
            .iter()
            .filter(|a| !allowed.iter().any(|g| g.eq_ignore_ascii_case(&a.guid)))
            .map(|a| a.guid.clone())
            .collect();
        Self {
            exe_path: rule.exe_path.clone(),
            cols: [allowed, denied],
            allow_loopback: rule.allow_loopback,
            names: rule.known_names.clone(),
            original: rule.clone(),
            touched: false,
            activated: rule.configured,
        }
    }

    /// Write the current state into the rule (called after every change).
    pub fn apply(&self, rule: &mut Rule, adapters: &[Adapter]) {
        rule.allowed = self.cols[0].clone();
        rule.allow_loopback = self.allow_loopback;
        rule.configured = true;
        // remember names so that missing interfaces can still be shown by name
        rule.known_names.clear();
        for g in &rule.allowed {
            let name = adapters
                .iter()
                .find(|a| a.guid.eq_ignore_ascii_case(g))
                .map(|a| a.name.clone())
                .or_else(|| self.names.get(g).cloned());
            if let Some(n) = name {
                rule.known_names.insert(g.clone(), n);
            }
        }
    }

    /// Put the settings from when the view was opened back into the rule.
    pub fn restore_into(&self, rule: &mut Rule) {
        rule.allowed = self.original.allowed.clone();
        rule.allow_loopback = self.original.allow_loopback;
        rule.configured = self.original.configured;
        rule.known_names = self.original.known_names.clone();
    }

    /// Reset the view itself to the original state.
    pub fn revert(&mut self, adapters: &[Adapter]) {
        let original = self.original.clone();
        *self = Configure::new(&original, adapters);
    }

    fn allow_all(&mut self) {
        let mut denied = std::mem::take(&mut self.cols[1]);
        self.cols[0].append(&mut denied);
    }

    fn deny_all(&mut self, adapters: &[Adapter]) {
        let allowed = std::mem::take(&mut self.cols[0]);
        for g in allowed {
            // interfaces that no longer exist are simply dropped
            if adapters.iter().any(|a| a.guid.eq_ignore_ascii_case(&g)) {
                self.cols[1].push(g);
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, adapters: &[Adapter], icons: &mut IconCache) -> Action {
        let mut close = false;
        let mut revert = false;
        let mut changed = false;

        ui.horizontal(|ui| {
            if ui.button("< Back").on_hover_text("Close (changes are already applied)").clicked() {
                close = true;
            }
            ui.heading("Configure interfaces");
        });

        let file_name = Path::new(&self.exe_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.exe_path.clone());
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width() - 2.0);
            ui.horizontal(|ui| {
                icons.show(ui, &self.exe_path, 36.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&file_name).strong().size(16.0));
                    ui.label(RichText::new(&self.exe_path).small().weak());
                });
            });
        });

        ui.label(
            RichText::new(
                "Drag interfaces between the columns. Traffic goes through the first available interface \
                 in Allowed (top = highest priority); if it goes down, the next one is used. \
                 Denied interfaces are never used.",
            )
            .weak(),
        );

        if !self.activated {
            ui.label(
                RichText::new(
                    "This app is not configured yet, so its traffic is not controlled. \
                     Change anything below or click Activate to start applying this setup.",
                )
                .color(theme::ORANGE),
            );
            if ui.add(theme::primary("Activate")).clicked() {
                changed = true;
            }
        }

        ui.horizontal(|ui| {
            if ui.button("Allow all").on_hover_text("Move every interface to Allowed").clicked() {
                self.allow_all();
                changed = true;
            }
            if ui
                .button("Deny all")
                .on_hover_text("Move every interface to Denied (no network access)")
                .clicked()
            {
                self.deny_all(adapters);
                changed = true;
            }
            ui.separator();
            if ui
                .checkbox(&mut self.allow_loopback, "Allow loopback (127.0.0.1, ::1)")
                .on_hover_text("Connections to services running on this PC. Turn off to block them.")
                .changed()
            {
                changed = true;
            }
        });

        // ---- two columns ----
        let height = (ui.available_height() - 120.0).max(200.0);
        let mut from_to: Option<(Loc, Loc)> = None;
        ui.columns(2, |cols| {
            for (ci, col_ui) in cols.iter_mut().enumerate() {
                column(col_ui, ci, &self.cols[ci], adapters, &self.names, height, &mut from_to);
            }
        });
        if let Some((from, to)) = from_to {
            let before = self.cols.clone();
            self.move_item(from, to);
            if before != self.cols {
                changed = true;
            }
        }

        // ---- summary + buttons ----
        let active = self.cols[0]
            .iter()
            .find_map(|g| adapters.iter().find(|a| a.up && a.guid.eq_ignore_ascii_case(g)));
        match active {
            Some(a) => {
                ui.label(RichText::new(format!("Traffic will use: {}", a.display())).color(theme::GREEN));
            }
            None if self.cols[0].is_empty() => {
                ui.label(
                    RichText::new("No allowed interfaces: this app will have no network access.")
                        .color(theme::RED),
                );
            }
            None => {
                ui.label(
                    RichText::new(
                        "None of the allowed interfaces is up right now: network access stays blocked until one comes up.",
                    )
                    .color(theme::ORANGE),
                );
            }
        }
        ui.label(
            RichText::new("Changes are applied immediately. Revert undoes everything since you opened this screen.")
                .small()
                .weak(),
        );
        ui.horizontal(|ui| {
            if ui
                .add(theme::primary("Done"))
                .on_hover_text("Close this screen (changes are already applied)")
                .clicked()
            {
                close = true;
            }
            if ui
                .add_enabled(self.touched, egui::Button::new("Revert all changes"))
                .on_hover_text("Restore the settings from when this screen was opened")
                .clicked()
            {
                revert = true;
            }
        });

        if changed {
            self.touched = true;
            self.activated = true;
        }
        if revert {
            Action::Revert
        } else if close {
            Action::Close
        } else if changed {
            Action::Changed
        } else {
            Action::None
        }
    }

    fn move_item(&mut self, from: Loc, to: Loc) {
        if from.col > 1 || to.col > 1 || from.row >= self.cols[from.col].len() {
            return;
        }
        // "insert before" index in the list as it was before the removal
        let mut to_row = to.row.min(self.cols[to.col].len());
        let item = self.cols[from.col].remove(from.row);
        if from.col == to.col && from.row < to_row {
            to_row -= 1;
        }
        let to_row = to_row.min(self.cols[to.col].len());
        self.cols[to.col].insert(to_row, item);
    }
}

fn column(
    ui: &mut egui::Ui,
    ci: usize,
    items: &[String],
    adapters: &[Adapter],
    names: &HashMap<String, String>,
    height: f32,
    from_to: &mut Option<(Loc, Loc)>,
) {
    let (title, color, note) = if ci == 0 {
        ("Allowed", theme::GREEN, "top = first choice")
    } else {
        ("Denied", theme::RED, "never used")
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().size(16.0).color(color));
        ui.label(RichText::new(format!("{} - {}", items.len(), note)).small().weak());
    });

    let frame = egui::Frame::none()
        .fill(Color32::from_rgb(25, 27, 32))
        .stroke(Stroke::new(1.0_f32, theme::CARD_STROKE))
        .rounding(10.0)
        .inner_margin(egui::Margin::same(8.0));

    let (_, dropped) = ui.dnd_drop_zone::<Loc, ()>(frame, |ui| {
        ui.set_min_size(egui::vec2(ui.available_width() - 2.0, height));
        egui::ScrollArea::vertical()
            .id_salt(("cfg_col", ci))
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if items.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Drop interfaces here").weak());
                    });
                }
                for (row, guid) in items.iter().enumerate() {
                    let loc = Loc { col: ci, row };
                    let id = egui::Id::new(("iface", guid.as_str()));
                    let prio = if ci == 0 { Some(row + 1) } else { None };
                    let response = ui
                        .dnd_drag_source(id, loc, |ui| card(ui, guid, adapters, names, prio))
                        .response;

                    // Insertion marker + drop handling
                    if let (Some(pointer), Some(hovered)) = (
                        ui.input(|i| i.pointer.interact_pos()),
                        response.dnd_hover_payload::<Loc>(),
                    ) {
                        let rect = response.rect;
                        let stroke = Stroke::new(2.0_f32, theme::ACCENT);
                        let insert_row = if *hovered == loc {
                            ui.painter().hline(rect.x_range(), rect.center().y, stroke);
                            row
                        } else if pointer.y < rect.center().y {
                            ui.painter().hline(rect.x_range(), rect.top(), stroke);
                            row
                        } else {
                            ui.painter().hline(rect.x_range(), rect.bottom(), stroke);
                            row + 1
                        };
                        if let Some(dragged) = response.dnd_release_payload::<Loc>() {
                            *from_to = Some((*dragged, Loc { col: ci, row: insert_row }));
                        }
                    }
                }
            });
    });

    // Dropped on the column but not on an item: append to the end
    if let Some(dragged) = dropped {
        *from_to = Some((*dragged, Loc { col: ci, row: usize::MAX }));
    }
}

fn card(
    ui: &mut egui::Ui,
    guid: &str,
    adapters: &[Adapter],
    names: &HashMap<String, String>,
    prio: Option<usize>,
) {
    let adapter = adapters.iter().find(|a| a.guid.eq_ignore_ascii_case(guid));
    let w = ui.available_width();
    egui::Frame::none()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0_f32, theme::CARD_STROKE))
        .rounding(8.0)
        .inner_margin(egui::Margin::symmetric(10.0, 7.0))
        .show(ui, |ui| {
            ui.set_min_width((w - 22.0).max(100.0));
            ui.horizontal(|ui| {
                if let Some(p) = prio {
                    ui.label(RichText::new(p.to_string()).strong().size(16.0).color(theme::ACCENT));
                }
                ui.vertical(|ui| match adapter {
                    Some(a) => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&a.name).strong());
                            theme::chip(ui, a.kind(), theme::GRAY);
                        });
                        let ips = if a.ipv4.is_empty() {
                            "no IPv4 address".to_string()
                        } else {
                            a.ipv4.join(", ")
                        };
                        ui.label(RichText::new(ips).small().weak());
                    }
                    None => {
                        let title = names.get(guid).map(String::as_str).unwrap_or("Unknown interface");
                        ui.label(RichText::new(title).strong().color(theme::GRAY));
                        let tail = guid.get(guid.len().saturating_sub(9)..).unwrap_or(guid);
                        ui.label(RichText::new(format!("not present now (...{tail})")).small().weak());
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| match adapter {
                    Some(a) if a.up => theme::chip(ui, "Up", theme::GREEN),
                    Some(_) => theme::chip(ui, "Down", theme::ORANGE),
                    None => theme::chip(ui, "Not present", theme::GRAY),
                });
            });
        });
}
