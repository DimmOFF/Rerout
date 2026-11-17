//! Main list: applications as cards (compact or expanded), manual order and groups.
//!
//! Drag & drop starts ONLY from the grip on the left of a card / group header (so that buttons and
//! checkboxes inside the card keep working). A card can be dropped before / after another card (it joins that card's
//! group), onto a group header (goes to the end of the group) or onto the drop zone at the
//! end of the list (leaves its group). Group headers can be dragged to reorder groups.

use crate::{conns, icons::IconCache, theme};
use eframe::egui::{self, Align, Layout, RichText, Stroke};
use rerout_common::adapters::Adapter;
use rerout_common::config::{self, Config, Dest, Group, Rule};
use std::collections::HashMap;
use std::path::Path;

/// Width of the button block of an expanded card / of a compact card.
const BUTTONS_W: f32 = 224.0;
const BUTTONS_W_COMPACT: f32 = 268.0;
const BUTTON_MIN_W: f32 = 108.0;
const COMPACT_BUTTON_W: f32 = 96.0;
/// Width of the warning triangle in pixels (everything inside is scaled from it).
const WARN_ICON_SIZE: f32 = 12.0;

/// What is being dragged.
#[derive(Clone, Copy, PartialEq)]
enum Drag {
    /// an application (index in `Config::rules`)
    Rule(usize),
    /// a group header (group id)
    Group(u32),
}

/// UI state that lives between frames (inline group renaming).
#[derive(Default)]
pub struct State {
    pub renaming: Option<u32>,
    pub rename_buf: String,
    pub rename_fresh: bool,
}

#[derive(Default)]
pub struct Acts {
    pub changed: bool,
    pub remove: Option<usize>,
    pub launch: Option<usize>,
    pub details: Option<usize>,
    pub configure: Option<usize>,
    pub move_rule: Option<(usize, Dest)>,
    /// (group to move, target group, place after the target)
    pub move_group: Option<(u32, u32, bool)>,
    /// group to move to the end of the group list
    pub move_group_end: Option<u32>,
    /// (rule index, group id; 0 = no group)
    pub set_group: Option<(usize, u32)>,
    pub toggle_group: Option<u32>,
    pub rename_group: Option<(u32, String)>,
    pub delete_group: Option<u32>,
}

/// Read-only data shared by all cards.
struct Ctx<'a> {
    adapters: &'a [Adapter],
    groups: &'a [Group],
    sandboxed: &'a HashMap<String, usize>,
    summaries: &'a HashMap<String, conns::Summary>,
    compact: bool,
}

/// Per-application values computed once per frame.
struct Item<'a> {
    file_name: String,
    resolved: Option<&'a Adapter>,
    failed: usize,
    sum: conns::Summary,
    browser: bool,
}

pub fn ui(
    ui: &mut egui::Ui,
    cfg: &mut Config,
    adapters: &[Adapter],
    icons: &mut IconCache,
    sandboxed: &HashMap<String, usize>,
    summaries: &HashMap<String, conns::Summary>,
    state: &mut State,
) -> Acts {
    let mut acts = Acts::default();
    let Config { rules, groups, compact, .. } = cfg;
    let groups: &[Group] = groups;

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

    let cx = Ctx {
        adapters,
        groups,
        sandboxed,
        summaries,
        compact: *compact,
    };

    // Which application is shown where (an unknown group id counts as "no group")
    let mut ungrouped: Vec<usize> = Vec::new();
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); groups.len()];
    for (i, r) in rules.iter().enumerate() {
        match groups.iter().position(|g| g.id == r.group) {
            Some(gi) => members[gi].push(i),
            None => ungrouped.push(i),
        }
    }

    let dragging = egui::DragAndDrop::has_payload_of_type::<Drag>(ui.ctx());
    // While a GROUP is being dragged everything collapses to the headers: that makes every group
    // an easy drop target (before / after it) however long the list is.
    let dragging_group = matches!(
        egui::DragAndDrop::payload::<Drag>(ui.ctx()).as_deref(),
        Some(Drag::Group(_))
    );

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // Groups come first
        for (gi, g) in groups.iter().enumerate() {
            group_header(ui, g, members[gi].len(), state, &mut acts);
            if !g.collapsed && !dragging_group {
                for &i in &members[gi] {
                    ui.push_id(i, |ui| row(ui, &cx, i, &mut rules[i], icons, &mut acts));
                }
            }
        }

        // Applications without a group below the groups
        if !groups.is_empty() && (!ungrouped.is_empty() || dragging) {
            ungrouped_header(ui, ungrouped.len(), &mut acts);
        }
        if !dragging_group {
            for &i in &ungrouped {
                ui.push_id(i, |ui| row(ui, &cx, i, &mut rules[i], icons, &mut acts));
            }
        }

        // While dragging: a drop zone at the end of the list
        if dragging {
            ui.add_space(6.0);
            let frame = egui::Frame::none()
                .stroke(Stroke::new(1.0_f32, theme::CARD_STROKE))
                .rounding(8.0)
                .inner_margin(egui::Margin::same(10.0));
            let (_, dropped) = ui.dnd_drop_zone::<Drag, ()>(frame, |ui| {
                ui.set_min_width(ui.available_width() - 2.0);
                let text = if dragging_group {
                    "Drop here to move the group to the end"
                } else if groups.is_empty() {
                    "Drop here to move to the end"
                } else {
                    "Drop here to move to the end, without a group"
                };
                ui.label(RichText::new(text).weak());
            });
            if let Some(p) = dropped {
                match *p {
                    Drag::Rule(from) => acts.move_rule = Some((from, Dest::EndOfGroup(0))),
                    Drag::Group(id) => acts.move_group_end = Some(id),
                }
            }
        }
    });
    acts
}

// ----------------------------------------------------------------------------------------
// Headers
// ----------------------------------------------------------------------------------------

fn ungrouped_header(ui: &mut egui::Ui, count: usize, acts: &mut Acts) {
    ui.add_space(4.0);
    let resp = egui::Frame::none()
        .inner_margin(egui::Margin::symmetric(6.0, 4.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Ungrouped").strong().size(16.0).color(theme::GRAY));
                ui.label(RichText::new(format!("{count} app(s)")).small().weak());
            });
        })
        .response;
    if let Some(p) = resp.dnd_hover_payload::<Drag>() {
        ui.painter().rect_stroke(resp.rect, 8.0, Stroke::new(2.0_f32, theme::ACCENT));
        if resp.dnd_release_payload::<Drag>().is_some() {
            match *p {
                // an application leaves its group
                Drag::Rule(from) => acts.move_rule = Some((from, Dest::EndOfGroup(0))),
                // a group goes to the end of the group list (the ungrouped ones are always below)
                Drag::Group(id) => acts.move_group_end = Some(id),
            }
        }
    }
}

fn group_header(ui: &mut egui::Ui, g: &Group, count: usize, state: &mut State, acts: &mut Acts) {
    ui.add_space(4.0);
    let resp = {
            egui::Frame::none()
                .fill(theme::GROUP_BG)
                .rounding(8.0)
                .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        drag_handle(ui, egui::Id::new(("grip", "group", g.id)), Drag::Group(g.id), &g.name);
                        if chevron(ui, !g.collapsed).clicked() {
                            acts.toggle_group = Some(g.id);
                        }
                        if state.renaming == Some(g.id) {
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut state.rename_buf).desired_width(240.0),
                            );
                            if state.rename_fresh {
                                edit.request_focus();
                                state.rename_fresh = false;
                            }
                            if edit.lost_focus() {
                                let name = state.rename_buf.trim().to_string();
                                let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                if !name.is_empty() && !cancelled {
                                    acts.rename_group = Some((g.id, name));
                                }
                                state.renaming = None;
                            }
                        } else {
                            let title = ui
                                .add(
                                    egui::Label::new(RichText::new(&g.name).strong().size(16.0))
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(if g.collapsed { "Expand the group" } else { "Collapse the group" });
                            if title.clicked() {
                                acts.toggle_group = Some(g.id);
                            }
                            ui.label(RichText::new(format!("{count} app(s)")).small().weak());
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.menu_button("...", |ui| {
                                if ui.button("Rename").clicked() {
                                    state.renaming = Some(g.id);
                                    state.rename_buf = g.name.clone();
                                    state.rename_fresh = true;
                                    ui.close_menu();
                                }
                                if ui
                                    .button(RichText::new("Delete group").color(theme::RED))
                                    .on_hover_text("The applications stay in the list, just without a group")
                                    .clicked()
                                {
                                    acts.delete_group = Some(g.id);
                                    ui.close_menu();
                                }
                            });
                        });
                    });
                })
                .response
        };

    // Drop targets: an application goes to the end of this group, a group header reorders groups
    if let (Some(pointer), Some(p)) = (
        ui.input(|i| i.pointer.interact_pos()),
        resp.dnd_hover_payload::<Drag>(),
    ) {
        match *p {
            Drag::Rule(from) => {
                ui.painter().rect_stroke(resp.rect, 8.0, Stroke::new(2.0_f32, theme::ACCENT));
                if resp.dnd_release_payload::<Drag>().is_some() {
                    acts.move_rule = Some((from, Dest::EndOfGroup(g.id)));
                }
            }
            Drag::Group(other) if other != g.id => {
                let after = pointer.y > resp.rect.center().y;
                let y = if after { resp.rect.bottom() } else { resp.rect.top() };
                ui.painter().hline(resp.rect.x_range(), y, Stroke::new(2.0_f32, theme::ACCENT));
                if resp.dnd_release_payload::<Drag>().is_some() {
                    acts.move_group = Some((other, g.id, after));
                }
            }
            _ => {}
        }
    }
}

/// Grip on the left of a card / group header. Only this handle starts a drag, so everything
/// else in the card keeps working as normal buttons. While dragging, a small label with the
/// name follows the pointer.
fn drag_handle(ui: &mut egui::Ui, id: egui::Id, payload: Drag, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 28.0), egui::Sense::hover());
    let resp = ui
        .interact(rect, id, egui::Sense::drag())
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text("Drag to reorder");

    if ui.is_rect_visible(rect) {
        let color = if resp.hovered() || resp.dragged() { theme::ACCENT } else { theme::GRAY };
        let c = rect.center();
        for dy in [-6.0_f32, 0.0, 6.0] {
            for dx in [-2.5_f32, 2.5] {
                ui.painter().circle_filled(egui::pos2(c.x + dx, c.y + dy), 1.4, color);
            }
        }
    }

    if resp.dragged() {
        egui::DragAndDrop::set_payload(ui.ctx(), payload);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some(pos) = ui.ctx().pointer_interact_pos() {
            egui::Area::new(egui::Id::new("drag_ghost"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pos + egui::vec2(14.0, 10.0))
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(RichText::new(label).strong());
                    });
                });
        }
    }
}

/// Expand / collapse button with a drawn chevron (v = open, > = collapsed).
fn chevron(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&resp);
        ui.painter().rect_filled(rect, 8.0, visuals.weak_bg_fill);
        let c = rect.center();
        let points = if open {
            vec![
                egui::pos2(c.x - 5.0, c.y - 2.5),
                egui::pos2(c.x, c.y + 2.5),
                egui::pos2(c.x + 5.0, c.y - 2.5),
            ]
        } else {
            vec![
                egui::pos2(c.x - 2.5, c.y - 5.0),
                egui::pos2(c.x + 2.5, c.y),
                egui::pos2(c.x - 2.5, c.y + 5.0),
            ]
        };
        ui.painter()
            .add(egui::Shape::line(points, Stroke::new(2.0_f32, visuals.fg_stroke.color)));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ----------------------------------------------------------------------------------------
// Cards
// ----------------------------------------------------------------------------------------

fn row(ui: &mut egui::Ui, cx: &Ctx, i: usize, rule: &mut Rule, icons: &mut IconCache, acts: &mut Acts) {
    let key = config::norm_path(&rule.exe_path);
    let file_name = Path::new(&rule.exe_path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| rule.exe_path.clone());
    let browser = is_chromium(&file_name);
    let item = Item {
        file_name,
        resolved: config::resolve(rule, cx.adapters),
        failed: cx.sandboxed.get(&key).copied().unwrap_or(0),
        sum: cx.summaries.get(&key).copied().unwrap_or_default(),
        browser,
    };

    // Only the grip on the left starts a drag (see drag_handle); the rest of the card stays clickable
    let resp = if cx.compact {
        card_compact(ui, cx, &item, i, rule, icons, acts)
    } else {
        card_expanded(ui, cx, &item, i, rule, icons, acts)
    };

    // Drop target: before / after this card
    if let (Some(pointer), Some(p)) = (
        ui.input(|inp| inp.pointer.interact_pos()),
        resp.dnd_hover_payload::<Drag>(),
    ) {
        if let Drag::Rule(from) = *p {
            if from != i {
                let before = pointer.y < resp.rect.center().y;
                let y = if before { resp.rect.top() } else { resp.rect.bottom() };
                ui.painter().hline(resp.rect.x_range(), y, Stroke::new(2.0_f32, theme::ACCENT));
                if resp.dnd_release_payload::<Drag>().is_some() {
                    acts.move_rule = Some((from, if before { Dest::Before(i) } else { Dest::After(i) }));
                }
            }
        }
    }
}

fn card_expanded(
    ui: &mut egui::Ui,
    cx: &Ctx,
    item: &Item,
    i: usize,
    rule: &mut Rule,
    icons: &mut IconCache,
    acts: &mut Acts,
) -> egui::Response {
    theme::card().show(ui, |ui| {
        // minus the frame stroke (1 px per side), otherwise every card is 2 px wider than the previous one
        let total = ui.available_width() - 2.0;
        ui.set_width(total);

        // Two blocks with FIXED widths, so the buttons sit at the same x in every card
        let gap = ui.spacing().item_spacing.x;
        let left_w = (total - BUTTONS_W - gap).max(260.0);

        ui.horizontal_top(|ui| {
            // ---- left: checkbox, icon, information ----
            ui.allocate_ui_with_layout(egui::vec2(left_w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(left_w);
                ui.horizontal_top(|ui| {
                    drag_handle(ui, egui::Id::new(("grip", rule.exe_path.as_str())), Drag::Rule(i), &item.file_name);
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
                                    egui::Label::new(RichText::new(&item.file_name).strong().size(15.0))
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
                            if rule.configured && has_note(item) {
                                warn_icon(ui, item.sum, item.failed, item.browser);
                            }
                        });
                        ui.label(RichText::new(&rule.exe_path).small().weak());

                        if rule.configured {
                            info(ui, rule, cx.adapters, item.resolved);
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
                group_menu(ui, cx.groups, rule.group, i, acts);
            });
        });
    })
    .response
}

fn card_compact(
    ui: &mut egui::Ui,
    cx: &Ctx,
    item: &Item,
    i: usize,
    rule: &mut Rule,
    icons: &mut IconCache,
    acts: &mut Acts,
) -> egui::Response {
    theme::card_compact().show(ui, |ui| {
        let total = ui.available_width() - 2.0;
        ui.set_width(total);
        let gap = ui.spacing().item_spacing.x;
        let left_w = (total - BUTTONS_W_COMPACT - gap).max(200.0);

        ui.horizontal(|ui| {
            // ---- left: checkbox, icon, name, status, short summary ----
            ui.allocate_ui_with_layout(egui::vec2(left_w, 0.0), Layout::left_to_right(Align::Center), |ui| {
                ui.set_width(left_w);
                drag_handle(ui, egui::Id::new(("grip", rule.exe_path.as_str())), Drag::Rule(i), &item.file_name);
                if ui
                    .checkbox(&mut rule.enabled, "")
                    .on_hover_text("Enable / disable this rule")
                    .changed()
                {
                    acts.changed = true;
                }
                icons.show(ui, &rule.exe_path, 26.0);
                ui.horizontal_wrapped(|ui| {
                    let name = ui
                        .add(
                            egui::Label::new(RichText::new(&item.file_name).strong().size(15.0))
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(format!("{}\nClick to show open connections", rule.exe_path));
                    if name.clicked() {
                        acts.details = Some(i);
                    }
                    status_chip(ui, rule, item.resolved);
                    if rule.configured && has_note(item) {
                        warn_icon(ui, item.sum, item.failed, item.browser);
                    }
                    compact_summary(ui, rule, cx.adapters);
                });
            });

            // ---- right: the main buttons and a menu with the rest ----
            ui.allocate_ui_with_layout(
                egui::vec2(BUTTONS_W_COMPACT, 0.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_width(BUTTONS_W_COMPACT);
                    let min = egui::vec2(COMPACT_BUTTON_W, 0.0);
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
                    ui.menu_button("...", |ui| {
                        if ui.button("Connections").clicked() {
                            acts.details = Some(i);
                            ui.close_menu();
                        }
                        group_menu(ui, cx.groups, rule.group, i, acts);
                        ui.separator();
                        if ui.button(RichText::new("Remove").color(theme::RED)).clicked() {
                            acts.remove = Some(i);
                            ui.close_menu();
                        }
                    });
                },
            );
        });
    })
    .response
}

/// "Move to group" menu (as a button in the expanded card, as a submenu in the compact one).
fn group_menu(ui: &mut egui::Ui, groups: &[Group], rule_group: u32, i: usize, acts: &mut Acts) {
    if groups.is_empty() {
        return;
    }
    ui.menu_button("Move to group", |ui| {
        if rule_group != 0 && ui.button("No group").clicked() {
            acts.set_group = Some((i, 0));
            ui.close_menu();
        }
        for g in groups {
            if g.id != rule_group && ui.button(&g.name).clicked() {
                acts.set_group = Some((i, g.id));
                ui.close_menu();
            }
        }
    });
}

/// The one chip that matters most: what is happening with this app right now.
fn status_chip(ui: &mut egui::Ui, rule: &Rule, resolved: Option<&Adapter>) {
    if !rule.configured {
        theme::chip(ui, "Not configured", theme::ORANGE);
    } else if !rule.enabled {
        theme::chip(ui, "Disabled", theme::GRAY);
    } else {
        match resolved {
            Some(a) => theme::chip(ui, format!("via {}", a.name), theme::GREEN),
            None => theme::chip(ui, "Blocked: no allowed interface is up", theme::RED),
        }
    }
}

/// One short line of text for the compact view.
fn compact_summary(ui: &mut egui::Ui, rule: &Rule, adapters: &[Adapter]) {
    if !rule.configured {
        ui.label(RichText::new("Click Configure to choose interfaces").small().weak());
        return;
    }
    let denied = adapters.iter().filter(|a| !rule.is_allowed(&a.guid)).count();
    ui.label(
        RichText::new(format!(
            "{} allowed, {} denied, loopback {}",
            rule.allowed.len(),
            denied,
            if rule.allow_loopback { "allowed" } else { "blocked" }
        ))
        .small()
        .weak(),
    );
}

/// Full read-only summary of the rule (expanded view).
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

// ----------------------------------------------------------------------------------------
// Warning icon
// ----------------------------------------------------------------------------------------

const CHROMIUM_FLAG: &str = "--enable-features=NetworkServiceInProcess";

/// Well-known Chromium-based browsers (their network code runs in a sandboxed process).
fn is_chromium(file_name: &str) -> bool {
    const NAMES: [&str; 8] = [
        "brave.exe",
        "chrome.exe",
        "msedge.exe",
        "vivaldi.exe",
        "opera.exe",
        "chromium.exe",
        "browser.exe",
        "yandex.exe",
    ];
    let n = file_name.to_lowercase();
    NAMES.contains(&n.as_str())
}

/// Is there anything to tell about this application (so that the icon is shown)?
fn has_note(item: &Item) -> bool {
    item.sum.denied > 0 || item.sum.local > 0 || item.failed > 0 || item.browser
}

/// Warning triangle with "!".
/// Orange: connections on not-allowed interfaces, or processes that cannot be controlled.
/// Gray (muted): only informational things (open loopback sockets, Chromium tip).
fn warn_icon(ui: &mut egui::Ui, sum: conns::Summary, failed: usize, browser: bool) {
    let problem = sum.denied > 0 || failed > 0;
    let k = WARN_ICON_SIZE;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(k + 2.0, k + 2.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let c = if problem { theme::ORANGE } else { theme::GRAY };
        let m = rect.center();
        let alpha = if resp.hovered() { 0.40 } else { 0.22 };
        // warning triangle
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(m.x, m.y - 0.46 * k),
                egui::pos2(m.x + 0.50 * k, m.y + 0.40 * k),
                egui::pos2(m.x - 0.50 * k, m.y + 0.40 * k),
            ],
            c.gamma_multiply(alpha),
            Stroke::new(0.10 * k, c),
        ));
        // exclamation mark: stem + dot
        ui.painter().line_segment(
            [egui::pos2(m.x, m.y - 0.14 * k), egui::pos2(m.x, m.y + 0.10 * k)],
            Stroke::new(0.12 * k, c),
        );
        ui.painter().circle_filled(egui::pos2(m.x, m.y + 0.25 * k), 0.07 * k, c);
    }
    let resp = resp.on_hover_ui(|ui| {
        ui.set_max_width(380.0);
        if sum.denied > 0 {
            ui.label(format!(
                "{} connection(s) of this app are on an interface that is not allowed. Open Connections to see them; \
                 Close makes the app reconnect through the current rule.",
                sum.denied
            ));
        }
        if sum.local > 0 {
            ui.label(format!(
                "{} local / loopback socket(s) are open. They are not affected by the interface rules.",
                sum.local
            ));
        }
        if failed > 0 {
            ui.label(format!(
                "{failed} process(es) of this app could not load Rerout's DLL (sandboxed or protected), \
                 so their traffic is not controlled."
            ));
        }
        if browser {
            ui.label(
                "Chromium browsers run their network code in a sandboxed process that cannot load the DLL. \
                 Start the browser with this flag to run it in the main process instead \
                 (documented by Chromium; newer versions may ignore it):",
            );
            ui.label(RichText::new(CHROMIUM_FLAG).monospace());
            ui.label(RichText::new("Click the icon to copy the flag.").weak());
        }
    });
    if browser && resp.clicked() {
        ui.output_mut(|o| o.copied_text = CHROMIUM_FLAG.to_string());
    }
}
