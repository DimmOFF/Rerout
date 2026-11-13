//! Main list: one informational card per application.

use crate::{icons::IconCache, theme};
use eframe::egui::{self, Align, Layout, RichText};
use rerout_common::adapters::Adapter;
use rerout_common::config::{self, Rule};
use std::path::Path;

/// Width of the button block on the right side of every card.
const BUTTONS_W: f32 = 224.0;
const BUTTON_MIN_W: f32 = 108.0;

#[derive(Default)]
pub struct Acts {
    pub changed: bool,
    pub remove: Option<usize>,
    pub launch: Option<usize>,
    pub details: Option<usize>,
    pub configure: Option<usize>,
}

pub fn ui(ui: &mut egui::Ui, rules: &mut [Rule], adapters: &[Adapter], icons: &mut IconCache) -> Acts {
    let mut acts = Acts::default();

    if rules.is_empty() {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No applications yet").size(20.0));
            ui.add_space(4.0);
            ui.label(RichText::new("Use \"+ Running apps\" to pick a process,").weak());
            ui.label(RichText::new("\"+ Browse...\" to choose an .exe,").weak());
            ui.label(RichText::new("or drag & drop an .exe or a shortcut onto this window.").weak());
        });
        return acts;
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (i, rule) in rules.iter_mut().enumerate() {
            ui.push_id(i, |ui| row(ui, i, rule, adapters, icons, &mut acts));
        }
    });
    acts
}

fn row(ui: &mut egui::Ui, i: usize, rule: &mut Rule, adapters: &[Adapter], icons: &mut IconCache, acts: &mut Acts) {
    let resolved = config::resolve(rule, adapters);
    let file_name = Path::new(&rule.exe_path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| rule.exe_path.clone());

    theme::card().show(ui, |ui| {
        let total = ui.available_width();
        ui.set_width(total);

        // Two blocks with FIXED widths, so the buttons sit at the same x in every card
        let gap = ui.spacing().item_spacing.x;
        let left_w = (total - BUTTONS_W - gap).max(260.0);

        ui.horizontal_top(|ui| {
            // ---- left: checkbox, icon, information ----
            ui.allocate_ui_with_layout(egui::vec2(left_w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(left_w);
                ui.horizontal_top(|ui| {
                    if ui
                        .checkbox(&mut rule.enabled, "")
                        .on_hover_text("Enable / disable this rule")
                        .changed()
                    {
                        acts.changed = true;
                    }
                    icons.show(ui, &rule.exe_path, 32.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let name = ui
                                .add(
                                    egui::Label::new(RichText::new(&file_name).strong().size(15.0))
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("Show open connections");
                            if name.clicked() {
                                acts.details = Some(i);
                            }
                            if !rule.configured {
                                theme::chip(ui, "Not configured", theme::ORANGE);
                            } else if !rule.enabled {
                                theme::chip(ui, "Disabled", theme::GRAY);
                            }
                        });
                        ui.label(RichText::new(&rule.exe_path).small().weak());

                        if rule.configured {
                            info(ui, rule, adapters, resolved);
                        } else {
                            ui.label(
                                RichText::new("Click Configure to choose which interfaces this app may use.")
                                    .small()
                                    .color(theme::ORANGE),
                            );
                        }
                    });
                });
            });

            // ---- right: buttons (fixed width) ----
            ui.allocate_ui_with_layout(egui::vec2(BUTTONS_W, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(BUTTONS_W);
                let min = egui::vec2(BUTTON_MIN_W, 0.0);
                ui.horizontal(|ui| {
                    if ui.add(theme::primary("Configure").min_size(min)).clicked() {
                        acts.configure = Some(i);
                    }
                    let can_launch = rule.enabled && rule.configured;
                    if ui
                        .add_enabled(can_launch, egui::Button::new("Launch").min_size(min))
                        .on_hover_text("Start the app with the rule applied from the first packet")
                        .clicked()
                    {
                        acts.launch = Some(i);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("Connections").min_size(min)).clicked() {
                        acts.details = Some(i);
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("Remove").color(theme::RED)).min_size(min))
                        .clicked()
                    {
                        acts.remove = Some(i);
                    }
                });
            });
        });
    });
}

/// Read-only summary of the rule.
fn info(ui: &mut egui::Ui, rule: &Rule, adapters: &[Adapter], resolved: Option<&Adapter>) {
    // Allowed (priority order)
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Allowed:").small().weak());
        if rule.allowed.is_empty() {
            theme::chip(ui, "none", theme::RED);
        }
        for (n, guid) in rule.allowed.iter().enumerate() {
            let found = adapters.iter().find(|a| a.guid.eq_ignore_ascii_case(guid));
            let (text, color) = match found {
                Some(a) if a.up => (format!("{}. {}", n + 1, a.name), theme::GREEN),
                Some(a) => (format!("{}. {} (down)", n + 1, a.name), theme::GRAY),
                None => {
                    let name = rule
                        .known_names
                        .get(guid)
                        .map(String::as_str)
                        .unwrap_or("unknown interface");
                    (format!("{}. {} (not present)", n + 1, name), theme::GRAY)
                }
            };
            theme::chip(ui, text, color);
        }
    });

    // Denied = every present interface that is not allowed
    let denied: Vec<&Adapter> = adapters.iter().filter(|a| !rule.is_allowed(&a.guid)).collect();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Denied:").small().weak());
        if denied.is_empty() {
            theme::chip(ui, "none", theme::GRAY);
        }
        for a in denied {
            theme::chip(ui, a.name.clone(), theme::RED);
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Loopback:").small().weak());
        if rule.allow_loopback {
            theme::chip(ui, "allowed", theme::GREEN);
        } else {
            theme::chip(ui, "blocked", theme::RED);
        }
        if rule.enabled {
            match resolved {
                Some(a) => {
                    ui.label(
                        RichText::new(format!("   Using now: {}", a.display()))
                            .small()
                            .color(theme::GREEN),
                    );
                }
                None => {
                    ui.label(
                        RichText::new("   No allowed interface is up: network access is blocked")
                            .small()
                            .color(theme::RED),
                    );
                }
            }
        }
    });
}
