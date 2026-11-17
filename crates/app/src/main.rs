#![windows_subsystem = "windows"]

mod autostart;
mod configure;
mod conns;
mod elevate;
mod icons;
mod list;
mod logo;
mod picker;
mod theme;
mod tray;
mod version;
mod watcher;

use eframe::egui::{self, Align, Color32, Layout, RichText};
use rerout_common::adapters::{self, Adapter};
use rerout_common::config::{self, Config, Rule};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() -> eframe::Result {
    // Started by "Restart as administrator": wait until the old instance has exited
    elevate::wait_for_parent();

    // Already running (maybe in the tray): show that window and exit
    if !tray::acquire_single_instance() {
        tray::show_main_window();
        return Ok(());
    }

    autostart::migrate_legacy();
    // Refresh the autostart path in case the exe was moved
    if autostart::is_enabled() {
        autostart::set(true);
    }
    let tray_ok = tray::create(autostart::is_enabled());
    // Start hidden only when the tray works, otherwise the window could not be restored
    let minimized = tray_ok && std::env::args().any(|a| a == "--minimized");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(tray::WINDOW_TITLE)
            .with_inner_size([1000.0, 640.0])
            .with_min_inner_size([760.0, 420.0])
            .with_icon(egui::IconData {
                rgba: logo::icon_rgba(256),
                width: 256,
                height: 256,
            })
            .with_visible(!minimized),
        ..Default::default()
    };
    eframe::run_native(
        "Rerout",
        options,
        Box::new(move |cc| {
            theme::setup(&cc.egui_ctx);
            Ok(Box::new(App::new(cc.egui_ctx.clone(), tray_ok)))
        }),
    )
}

enum View {
    List,
    Configure(configure::Configure),
    Picker(picker::Picker),
}

struct App {
    cfg: Config,
    adapters: Vec<Adapter>,
    last_refresh: Instant,
    shared_rules: Arc<Mutex<Vec<Rule>>>,
    seen: Arc<Mutex<HashSet<u32>>>,
    dist: PathBuf,
    events: Receiver<watcher::Event>,
    status: String,
    // connections window
    details_for: Option<String>,
    conns: Vec<conns::ConnRow>,
    conns_updated: Instant,
    conns_expected: String,
    hide_local: bool,
    /// running as administrator (needed to close TCP connections)
    elevated: bool,
    /// result of the last close action, shown in the Connections window
    conn_msg: String,
    tray_ok: bool,
    view: View,
    icons: icons::IconCache,
    /// normalized exe path -> number of processes that could not load the DLL
    sandboxed: HashMap<String, usize>,
    /// live per-application socket summary for the icon in the list (normalized exe path)
    summaries: HashMap<String, conns::Summary>,
    summary_updated: Instant,
    /// inline group renaming
    list_state: list::State,
    /// pid -> exe path cache used to attribute sockets to rules
    owner_paths: HashMap<u32, Option<String>>,
}

impl App {
    fn new(ctx: egui::Context, tray_ok: bool) -> Self {
        let cfg = config::load();
        let (tx, rx) = channel();
        let shared_rules = Arc::new(Mutex::new(cfg.rules.clone()));
        let seen = Arc::new(Mutex::new(HashSet::new()));
        let dist = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        watcher::spawn(shared_rules.clone(), seen.clone(), dist.clone(), tx, ctx);
        let mut app = Self {
            cfg,
            adapters: adapters::list(),
            last_refresh: Instant::now(),
            shared_rules,
            seen,
            dist,
            events: rx,
            status: "Ready".into(),
            details_for: None,
            conns: Vec::new(),
            conns_updated: Instant::now(),
            conns_expected: String::new(),
            hide_local: true,
            elevated: elevate::is_elevated(),
            conn_msg: String::new(),
            tray_ok,
            view: View::List,
            icons: icons::IconCache::default(),
            sandboxed: HashMap::new(),
            summaries: HashMap::new(),
            summary_updated: Instant::now(),
            list_state: list::State::default(),
            owner_paths: HashMap::new(),
        };
        app.remember_names();
        app.refresh_summaries();
        app
    }

    fn save(&mut self) {
        if let Err(e) = config::save(&self.cfg) {
            self.status = format!("Could not save settings: {e}");
        }
        *self.shared_rules.lock().unwrap() = self.cfg.rules.clone();
    }

    fn refresh_adapters(&mut self) {
        self.adapters = adapters::list();
        self.last_refresh = Instant::now();
        self.remember_names();
    }

    /// Keep the last known name of every allowed interface in the rule, so that an
    /// interface that is missing right now is still shown by name. Present adapters
    /// give the name directly; for missing ones the registry is asked once.
    fn remember_names(&mut self) {
        let mut changed = false;
        for rule in &mut self.cfg.rules {
            for guid in rule.allowed.clone() {
                let present = self
                    .adapters
                    .iter()
                    .find(|a| a.guid.eq_ignore_ascii_case(&guid));
                match present {
                    Some(a) => {
                        if rule.known_names.get(&guid) != Some(&a.name) {
                            rule.known_names.insert(guid.clone(), a.name.clone());
                            changed = true;
                        }
                    }
                    None => {
                        if !rule.known_names.contains_key(&guid) {
                            if let Some(n) = adapters::registry_name(&guid) {
                                rule.known_names.insert(guid.clone(), n);
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        if changed {
            self.save();
        }
    }

    /// Count live sockets of every rule's application (denied / loopback) for the list icons.
    fn refresh_summaries(&mut self) {
        self.summary_updated = Instant::now();
        let keys: Vec<String> = self
            .cfg
            .rules
            .iter()
            .map(|r| config::norm_path(&r.exe_path))
            .collect();
        if keys.is_empty() {
            self.summaries.clear();
            return;
        }
        let owner = watcher::process_trees(&keys, &mut self.owner_paths);
        let allowed: Vec<Option<Vec<String>>> = self
            .cfg
            .rules
            .iter()
            .map(|r| {
                if r.enabled && r.configured {
                    Some(r.allowed.clone())
                } else {
                    None
                }
            })
            .collect();
        let sums = conns::summarize(&owner, &self.adapters, &allowed);
        self.summaries = keys.into_iter().zip(sums).collect();
    }

    fn rule_index(&self, path: &str) -> Option<usize> {
        let key = config::norm_path(path);
        self.cfg
            .rules
            .iter()
            .position(|r| config::norm_path(&r.exe_path) == key)
    }

    /// Adds a rule (not configured yet) or returns the index of the existing one.
    fn add_rule(&mut self, path: String) -> usize {
        if let Some(i) = self.rule_index(&path) {
            return i;
        }
        self.cfg.rules.push(Rule::new(path));
        self.save();
        self.cfg.rules.len() - 1
    }

    fn configure_view(&self, idx: usize) -> View {
        View::Configure(configure::Configure::new(&self.cfg.rules[idx], &self.adapters))
    }

    /// Called after every change in the Configure view: apply and save right away.
    fn apply_configure(&mut self, c: &configure::Configure) {
        if let Some(i) = self.rule_index(&c.exe_path) {
            c.apply(&mut self.cfg.rules[i], &self.adapters);
            self.save();
            self.status = "Changes applied".into();
        }
    }

    /// Undo everything since the Configure view was opened.
    fn revert_configure(&mut self, c: &mut configure::Configure) {
        if let Some(i) = self.rule_index(&c.exe_path) {
            c.restore_into(&mut self.cfg.rules[i]);
            self.save();
        }
        c.revert(&self.adapters);
        self.status = "Reverted to the settings from when this screen was opened".into();
    }

    /// File dialog. Returns the index of the added/selected rule.
    fn browse_exe(&mut self) -> Option<usize> {
        let path = rfd::FileDialog::new()
            .add_filter("Applications", &["exe", "lnk"])
            .pick_file()?;
        self.add_dropped(vec![path])
    }

    /// Adds .exe files or shortcuts (.lnk -> their target).
    /// Returns the index of the last rule that was added or already existed.
    fn add_dropped(&mut self, paths: Vec<PathBuf>) -> Option<usize> {
        let mut last = None;
        let mut added = 0;
        let mut problems: Vec<String> = Vec::new();

        for path in paths {
            match resolve_dropped(&path) {
                Ok(exe) => {
                    let exe_str = exe.to_string_lossy().into_owned();
                    let name = exe
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| exe_str.clone());
                    let existed = self.rule_index(&exe_str).is_some();
                    last = Some(self.add_rule(exe_str));
                    if existed {
                        problems.push(format!("{name} is already in the list"));
                    } else {
                        added += 1;
                    }
                }
                Err(e) => problems.push(e),
            }
        }

        self.status = match (added, problems.is_empty()) {
            (0, true) => return last,
            (0, false) => problems.join("; "),
            (n, true) => format!("Added {n} application(s)"),
            (n, false) => format!("Added {n}. {}", problems.join("; ")),
        };
        last
    }

    /// Refresh the connection table for the application at `path`.
    fn refresh_conns(&mut self, path: &str) {
        let key = config::norm_path(path);
        let rule = self
            .cfg
            .rules
            .iter()
            .find(|r| config::norm_path(&r.exe_path) == key);
        let enforced = rule.filter(|r| r.enabled && r.configured);

        self.conns_expected = match (rule, enforced) {
            (_, Some(r)) => match config::resolve(r, &self.adapters) {
                Some(a) => a.display(),
                None => "none available (network blocked)".into(),
            },
            (Some(r), None) if !r.configured => "rule is not configured".into(),
            _ => "rule is disabled".into(),
        };
        let allowed: Option<Vec<String>> = enforced.map(|r| r.allowed.clone());
        let pids = watcher::process_tree(&key);
        self.conns = conns::collect(&pids, &self.adapters, allowed.as_deref());
        self.conns_updated = Instant::now();
    }

    fn launch(&mut self, i: usize) {
        self.save(); // the hook DLL reads the config from disk
        let rule = self.cfg.rules[i].clone();
        let proc = match rerout_inject::spawn_suspended(Path::new(&rule.exe_path)) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Could not start: {e}");
                return;
            }
        };
        // so that the watcher does not inject a second time
        self.seen.lock().unwrap().insert(proc.pid);
        match rerout_inject::inject(proc.pid, &self.dist) {
            Ok(()) => {
                let pid = proc.pid;
                proc.resume();
                let up = config::resolve(&rule, &self.adapters).is_some();
                self.status = if up {
                    format!("Started (PID {pid})")
                } else {
                    format!("Started (PID {pid}), but no allowed interface is up: the app has no network access")
                };
            }
            Err(e) => {
                proc.kill(); // never let traffic bypass the rule
                self.status = format!("Could not attach to the process, start cancelled: {e}");
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                watcher::Event::Info(m) => self.status = m,
                watcher::Event::Sandboxed { key, name } => {
                    let count = self.sandboxed.entry(key).or_insert(0);
                    *count += 1;
                    let n = *count;
                    self.status = format!(
                        "{name}: {n} process(es) could not load the DLL (sandboxed or protected). See the hint on its card."
                    );
                }
            }
        }

        // Close button: minimize to the tray instead of quitting
        if self.tray_ok && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.details_for = None; // closes the connections window too
            tray::hide_main_window();
        }

        // Files dropped onto the window (.exe / .lnk)
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            if let Some(i) = self.add_dropped(dropped) {
                self.view = self.configure_view(i);
            }
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop_overlay"),
            ));
            let rect = ctx.screen_rect();
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(170));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop an .exe or a shortcut to add it",
                egui::FontId::proportional(22.0),
                Color32::WHITE,
            );
        }

        if self.last_refresh.elapsed() > Duration::from_secs(5) {
            self.refresh_adapters();
        }
        ctx.request_repaint_after(Duration::from_secs(2));
        if matches!(self.view, View::List) && self.summary_updated.elapsed() > Duration::from_secs(2) {
            self.refresh_summaries();
        }

        // ---- top bar ----
        let in_list = matches!(self.view, View::List);
        let cur_compact = self.cfg.compact;
        let mut open_picker = false;
        let mut browse = false;
        let mut refresh = false;
        let mut new_group = false;
        let mut new_compact: Option<bool> = None;
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("Rerout");
                ui.label(RichText::new(format!("v{}", version::VERSION)).small().weak())
                    .on_hover_text(version::about());
                if in_list {
                    // the rightmost control is added first
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Refresh").on_hover_text("Refresh the interface list").clicked() {
                            refresh = true;
                        }
                        if ui
                            .button("+ Browse...")
                            .on_hover_text("Choose an .exe or a shortcut (or drag & drop it onto the window)")
                            .clicked()
                        {
                            browse = true;
                        }
                        if ui
                            .add(theme::primary("+ Running apps"))
                            .on_hover_text("Pick an application from the running processes")
                            .clicked()
                        {
                            open_picker = true;
                        }
                        if ui
                            .button("+ Group")
                            .on_hover_text("Create a group; drag applications into it")
                            .clicked()
                        {
                            new_group = true;
                        }
                        ui.separator();
                        if ui
                            .selectable_label(!cur_compact, "Expanded")
                            .on_hover_text("Full information about every application")
                            .clicked()
                        {
                            new_compact = Some(false);
                        }
                        if ui
                            .selectable_label(cur_compact, "Compact")
                            .on_hover_text("One line per application")
                            .clicked()
                        {
                            new_compact = Some(true);
                        }
                    });
                }
            });
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(&self.status).small().weak());
            ui.add_space(4.0);
        });

        // ---- central area: the current view ----
        let existing: HashSet<String> = self
            .cfg
            .rules
            .iter()
            .map(|r| config::norm_path(&r.exe_path))
            .collect();
        let mut view = std::mem::replace(&mut self.view, View::List);
        let mut next: Option<View> = None;
        let mut acts = list::Acts::default();
        let mut cfg_action = configure::Action::None;
        let mut pick_action = picker::Action::None;

        egui::CentralPanel::default().show(ctx, |ui| match &mut view {
            View::List => {
                acts = list::ui(
                    ui,
                    &mut self.cfg,
                    &self.adapters,
                    &mut self.icons,
                    &self.sandboxed,
                    &self.summaries,
                    &mut self.list_state,
                )
            }
            View::Configure(c) => cfg_action = c.ui(ui, &self.adapters, &mut self.icons),
            View::Picker(p) => pick_action = p.ui(ui, &self.adapters, &existing, &mut self.icons),
        });

        match &mut view {
            View::Configure(c) => match cfg_action {
                configure::Action::Changed => self.apply_configure(c),
                configure::Action::Revert => self.revert_configure(c),
                configure::Action::Close => next = Some(View::List),
                configure::Action::None => {}
            },
            View::Picker(_) => match pick_action {
                picker::Action::Back => next = Some(View::List),
                picker::Action::Add(path) => {
                    let i = self.add_rule(path);
                    next = Some(self.configure_view(i));
                }
                picker::Action::None => {}
            },
            View::List => {}
        }

        if acts.changed {
            self.save();
        }
        if let Some((from, dest)) = acts.move_rule {
            self.cfg.move_rule(from, dest);
            self.save();
        }
        if let Some((id, target, after)) = acts.move_group {
            self.cfg.move_group(id, target, after);
            self.save();
        }
        if let Some(id) = acts.move_group_end {
            self.cfg.move_group_to_end(id);
            self.save();
        }
        if let Some((i, group)) = acts.set_group {
            if i < self.cfg.rules.len() {
                self.cfg.rules[i].group = group;
                self.save();
            }
        }
        if let Some(id) = acts.toggle_group {
            if let Some(g) = self.cfg.groups.iter_mut().find(|g| g.id == id) {
                g.collapsed = !g.collapsed;
                self.save();
            }
        }
        if let Some((id, name)) = acts.rename_group {
            if let Some(g) = self.cfg.groups.iter_mut().find(|g| g.id == id) {
                g.name = name;
                self.save();
            }
        }
        if let Some(id) = acts.delete_group {
            self.cfg.delete_group(id);
            if self.list_state.renaming == Some(id) {
                self.list_state.renaming = None;
            }
            self.save();
        }
        if let Some(i) = acts.remove {
            self.cfg.rules.remove(i);
            self.save();
        }
        if let Some(i) = acts.launch {
            self.launch(i);
        }
        if let Some(i) = acts.configure {
            next = Some(self.configure_view(i));
        }
        if let Some(i) = acts.details {
            let path = self.cfg.rules[i].exe_path.clone();
            let already_open = self.details_for.is_some();
            self.refresh_conns(&path);
            self.details_for = Some(path);
            if already_open {
                // The window may be behind other windows or minimized: bring it to the front
                let id = egui::ViewportId::from_hash_of("conn_details");
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
            }
        }
        if refresh {
            self.refresh_adapters();
        }
        if let Some(compact) = new_compact {
            if compact != self.cfg.compact {
                self.cfg.compact = compact;
                self.save();
            }
        }
        if new_group {
            let id = self.cfg.new_group("New group");
            self.save();
            // let the user type the name right away
            self.list_state.renaming = Some(id);
            self.list_state.rename_buf = "New group".into();
            self.list_state.rename_fresh = true;
        }
        if open_picker {
            next = Some(View::Picker(picker::Picker::new(&self.adapters)));
        }
        if browse {
            if let Some(i) = self.browse_exe() {
                next = Some(self.configure_view(i));
            }
        }
        self.view = next.unwrap_or(view);

        // ---- connections window ----
        if let Some(path) = self.details_for.clone() {
            if self.conns_updated.elapsed() > Duration::from_secs(1) {
                self.refresh_conns(&path);
            }
            let title = format!(
                "Connections - {}",
                Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            let mut open = true;
            let mut action = ConnAction::None;
            let elevated = self.elevated;
            let msg = self.conn_msg.clone();
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("conn_details"),
                egui::ViewportBuilder::default()
                    .with_title(title)
                    .with_inner_size([1020.0, 540.0]),
                |ctx, class| {
                    if ctx.input(|i| i.viewport().close_requested()) {
                        open = false;
                    }
                    if class == egui::ViewportClass::Embedded {
                        egui::Window::new("Connections").default_size([960.0, 500.0]).show(ctx, |ui| {
                            action = details_ui(ui, &self.conns, &self.conns_expected, &mut self.hide_local, elevated, &msg);
                        });
                    } else {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            action = details_ui(ui, &self.conns, &self.conns_expected, &mut self.hide_local, elevated, &msg);
                        });
                    }
                },
            );
            if !open {
                self.details_for = None;
            }
            self.handle_conn_action(action, &path);
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

/// What the user asked for in the Connections window.
#[derive(Clone, Copy)]
enum ConnAction {
    None,
    Close(conns::Target),
    CloseDenied,
    Elevate,
}

impl App {
    fn handle_conn_action(&mut self, action: ConnAction, path: &str) {
        match action {
            ConnAction::None => return,
            ConnAction::Close(t) => {
                self.conn_msg = match conns::close_tcp4(t) {
                    Ok(()) => "Connection closed. The app will reconnect using the current rule.".to_string(),
                    Err(e) => format!("Could not close the connection: {e}"),
                };
            }
            ConnAction::CloseDenied => {
                let targets: Vec<conns::Target> = self
                    .conns
                    .iter()
                    .filter(|r| r.verdict == conns::Verdict::Denied)
                    .filter_map(|r| r.target)
                    .collect();
                let mut closed = 0;
                let mut error = None;
                for t in targets {
                    match conns::close_tcp4(t) {
                        Ok(()) => closed += 1,
                        Err(e) => error = Some(e),
                    }
                }
                self.conn_msg = match error {
                    None => format!("Closed {closed} connection(s). The app will reconnect using the current rule."),
                    Some(e) => format!("Closed {closed}, but some failed: {e}"),
                };
            }
            ConnAction::Elevate => {
                if elevate::restart_as_admin() {
                    tray::quit(); // the new, elevated instance takes over
                }
                self.conn_msg = "The administrator restart was cancelled".into();
                return;
            }
        }
        self.refresh_conns(path);
    }
}


fn details_ui(
    ui: &mut egui::Ui,
    rows: &[conns::ConnRow],
    expected: &str,
    hide_local: &mut bool,
    elevated: bool,
    msg: &str,
) -> ConnAction {
    use conns::Verdict;
    let mut action = ConnAction::None;

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Active interface: {expected}")).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.checkbox(hide_local, "Hide LISTEN and loopback");
        });
    });

    let visible: Vec<&conns::ConnRow> = rows.iter().filter(|r| !(*hide_local && r.hideable)).collect();
    let ok = visible.iter().filter(|r| r.verdict == Verdict::Allowed).count();
    let bad = visible.iter().filter(|r| r.verdict == Verdict::Denied).count();
    ui.horizontal(|ui| {
        ui.label(format!("Shown: {}", visible.len()));
        ui.label(RichText::new(format!("allowed: {ok}")).color(theme::GREEN));
        ui.label(
            RichText::new(format!("not allowed: {bad}"))
                .color(if bad > 0 { theme::ORANGE } else { theme::GRAY }),
        );
    });

    // ---- actions ----
    let closable_bad = rows
        .iter()
        .filter(|r| r.verdict == Verdict::Denied && r.target.is_some())
        .count();
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                closable_bad > 0,
                egui::Button::new(format!("Close not-allowed connections ({closable_bad})")),
            )
            .on_hover_text("Reset every connection that is on a not-allowed interface; the app reconnects using the current rule")
            .clicked()
        {
            action = ConnAction::CloseDenied;
        }
        if !elevated {
            ui.label(
                RichText::new("Closing connections needs administrator rights.")
                    .small()
                    .color(theme::ORANGE),
            );
            if ui.button("Restart as administrator").clicked() {
                action = ConnAction::Elevate;
            }
        }
    });
    if !msg.is_empty() {
        ui.label(RichText::new(msg).small());
    }
    ui.label(
        RichText::new(
            "The interface is determined from the socket's local address. Connections opened before \
             the DLL was injected stay on their old interface: close them here and the app reconnects \
             through the current rule. Only IPv4 TCP connections can be closed (a Windows limitation). \
             Updates every second.",
        )
        .small()
        .weak(),
    );
    ui.separator();

    if visible.is_empty() {
        ui.label(RichText::new("No open connections (the app is not running or not using the network)").weak());
        return action;
    }

    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("conn_grid")
            .striped(true)
            .num_columns(8)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                for h in ["Process", "Proto", "Local address", "Remote address", "State", "Interface", "Rule", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();

                for r in visible {
                    let (text, color) = match r.verdict {
                        Verdict::Allowed => ("allowed", theme::GREEN),
                        Verdict::Denied => ("NOT allowed", theme::ORANGE),
                        Verdict::Unbound => ("not determined", theme::GRAY),
                        Verdict::Local => ("local", theme::GRAY),
                    };
                    ui.label(&r.proc);
                    ui.label(r.proto);
                    ui.label(&r.local);
                    ui.label(&r.remote);
                    ui.label(&r.state);
                    ui.label(&r.iface);
                    ui.label(RichText::new(text).color(color));
                    match r.target {
                        Some(t) => {
                            if ui
                                .small_button("Close")
                                .on_hover_text("Reset this connection; the app reconnects using the current rule")
                                .clicked()
                            {
                                action = ConnAction::Close(t);
                            }
                        }
                        None => {
                            ui.label("");
                        }
                    }
                    ui.end_row();
                }
            });
    });
    action
}

/// .exe -> as is; .lnk -> the shortcut's target.
fn resolve_dropped(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "exe" => Ok(path.to_path_buf()),
        "lnk" => lnk_target(path).map_err(|e| format!("{name}: {e}")),
        _ => Err(format!("{name}: an .exe or a shortcut (.lnk) is required")),
    }
}

/// Shortcut target via WScript.Shell (PowerShell is available on every Windows).
fn lnk_target(path: &Path) -> Result<PathBuf, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8; \
             (New-Object -ComObject WScript.Shell).CreateShortcut($env:REROUT_LNK).TargetPath",
        ])
        .env("REROUT_LNK", path) // the path goes through the environment: no quoting problems
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not read the shortcut: {e}"))?;

    let target = String::from_utf8_lossy(&out.stdout)
        .trim_start_matches('\u{feff}')
        .trim()
        .to_string();
    if target.is_empty() {
        return Err("the shortcut has no target".into());
    }
    if !target.to_lowercase().ends_with(".exe") {
        return Err(format!("the shortcut does not point to an .exe ({target})"));
    }
    Ok(PathBuf::from(target))
}
