use crate::{
    backup,
    config::{self, BUTTONS, Config, ENABLED, KEYS},
    device::{self, Info, Link},
    drawing,
    firmware::{self, Release, Target, Versions},
    macros::{self, Macro, Step},
    motion::{self, View},
    protocol::Result,
};
use eframe::egui::{self, Color32, Pos2, Stroke, Vec2};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
const GREEN: Color32 = Color32::from_rgb(117, 226, 164);
const TABS: [&str; 8] = [
    "Controller",
    "Buttons",
    "Sticks & triggers",
    "Rumble",
    "Motion",
    "Lighting",
    "Macros",
    "Device",
];

pub struct Session {
    pub config: Config,
    pub info: Info,
    pub theme: u8,
}
pub fn load() -> Result<Session> {
    let l = Link::connect()?;
    Ok(Session {
        config: l.read_config()?,
        info: l.info()?,
        theme: l.theme()?,
    })
}
fn refresh() -> Result<Event> {
    Ok(Event::Refreshed(load().ok(), firmware::versions()?))
}
enum Event {
    Loaded(Session),
    Message(String),
    Macro(Macro),
    Lighting(u8),
    Releases(Vec<Release>, Versions),
    Refreshed(Option<Session>, Versions),
    Failed(String),
}
pub struct App {
    tx: Sender<Event>,
    rx: Receiver<Event>,
    busy: bool,
    status: String,
    tab: usize,
    slot: usize,
    session: Option<Session>,
    draft: Option<Config>,
    name: String,
    theme: u8,
    motion: Arc<Mutex<View>>,
    motion_active: Arc<AtomicBool>,
    motion_render: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    macro_data: Macro,
    macro_index: usize,
    backup_path: String,
    target: Target,
    releases: Vec<Release>,
    release_index: usize,
    progress: Arc<Mutex<f32>>,
    confirm_flash: bool,
    versions: Versions,
    current_versions: Versions,
    info_generation: u64,
    flashing: bool,
    preview_levels: [f32; 2],
    follow_motion: bool,
    focus_button: Option<&'static str>,
    scroll_section: Option<usize>,
    auto_loaded_path: String,
    history: Vec<[f32; 3]>,
}
impl App {
    pub fn new(ctx: &egui::Context) -> Self {
        let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(24.));
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(16.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(16.));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(13.));
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(15.));
        style.spacing.item_spacing = Vec2::new(12., 10.);
        style.spacing.button_padding = Vec2::new(12., 7.);
        style.spacing.interact_size = Vec2::new(40., 34.);
        style.spacing.slider_width = 220.;
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(23, 28, 35);
        style.visuals.window_fill = Color32::from_rgb(30, 36, 44);
        style.visuals.override_text_color = Some(Color32::from_rgb(225, 231, 238));
        style.visuals.selection.bg_fill = Color32::from_rgb(43, 83, 70);
        style.visuals.selection.stroke = Stroke::new(1., GREEN);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(47, 57, 69);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(61, 74, 88);
        ctx.set_theme(egui::Theme::Dark);
        ctx.set_style_of(egui::Theme::Dark, style);
        let (tx, rx) = mpsc::channel();
        let motion = Arc::new(Mutex::new(View::default()));
        let active = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let motion_render = Arc::new(AtomicBool::new(false));
        motion::start(
            active.clone(),
            stop.clone(),
            motion.clone(),
            ctx.clone(),
            motion_render.clone(),
        );
        let mut app = Self {
            tx,
            rx,
            busy: false,
            status: "Connecting…".into(),
            tab: 0,
            slot: 0,
            session: None,
            draft: None,
            name: String::new(),
            theme: 5,
            motion,
            motion_active: active,
            motion_render,
            stop,
            cancel: Arc::new(AtomicBool::new(false)),
            macro_data: Macro::default(),
            macro_index: 0,
            backup_path: String::new(),
            target: Target::Controller,
            releases: vec![],
            release_index: 0,
            progress: Arc::new(Mutex::new(0.)),
            confirm_flash: false,
            versions: Versions::default(),
            current_versions: Versions::default(),
            info_generation: 0,
            flashing: false,
            preview_levels: [0.5; 2],
            follow_motion: false,
            focus_button: None,
            scroll_section: None,
            auto_loaded_path: String::new(),
            history: vec![],
        };
        if let Some(page) =
            std::env::args().find_map(|a| a.strip_prefix("--page=").map(str::to_owned))
        {
            app.tab = TABS
                .iter()
                .position(|t| t.eq_ignore_ascii_case(&page))
                .unwrap_or(0);
        }
        app.follow_motion = app.tab == 4;
        #[cfg(feature = "screenshots")]
        if std::env::var_os("U2_SCREENSHOT_TO").is_some() {
            let event = refresh().unwrap_or_else(Event::Failed);
            app.tx.send(event).ok();
            app.events();
            app.motion_active.store(true, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(if app.tab == 4 {
                1400
            } else {
                1000
            }));
            return app;
        }
        app.follow_motion = app.tab == 4;
        app.run(ctx, refresh);
        app
    }
    fn run(&mut self, ctx: &egui::Context, f: impl FnOnce() -> Result<Event> + Send + 'static) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "Working…".into();
        self.cancel.store(false, Ordering::Relaxed);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let event = f().unwrap_or_else(Event::Failed);
            let _ = tx.send(event);
            ctx.request_repaint();
        });
    }
    fn events(&mut self) {
        while let Ok(e) = self.rx.try_recv() {
            self.busy = false;
            self.flashing = false;
            self.motion.lock().unwrap().rumble = [0.; 2];
            match e {
                Event::Loaded(s) => self.loaded(s),
                Event::Refreshed(session, v) => {
                    self.merge_versions(v);
                    if let Some(s) = session {
                        self.loaded(s);
                    } else {
                        self.status =
                            "Device information refreshed · connect USB to edit profiles".into();
                    }
                }
                Event::Lighting(t) => {
                    self.theme = t;
                    if let Some(s) = &mut self.session {
                        s.theme = t;
                    }
                    self.status = "Lighting saved and verified".into();
                }
                Event::Message(s) => self.status = s,
                Event::Failed(s) => {
                    if let Some(session) = &self.session {
                        self.theme = session.theme;
                    }
                    if s.starts_with("Connect the Ultimate 2 directly")
                        && device::nodes().is_ok_and(|ns| ns.iter().any(|n| n.motion))
                    {
                        self.status="DInput connected · motion and rumble available · use USB to edit profiles".into();
                    } else {
                        self.status = format!("Error: {s}");
                    }
                }
                Event::Macro(m) => {
                    self.macro_data = m;
                    self.status = "Macro loaded".into();
                }
                Event::Releases(r, v) => {
                    self.merge_versions(v);
                    self.releases = r;
                    self.release_index = 0;
                    self.status = "Official stable firmware loaded".into();
                }
            }
        }
    }
    fn loaded(&mut self, s: Session) {
        self.versions.controller = Some(s.info.firmware as u32);
        self.current_versions.controller = Some(s.info.firmware as u32);
        self.auto_loaded_path = s.info.path.clone();
        self.theme = s.theme;
        self.name = s.config.name(self.slot);
        self.draft = Some(s.config.clone());
        self.status = "Connected · changes apply when you press Save profile".into();
        self.session = Some(s);
    }
    fn merge_versions(&mut self, v: Versions) {
        if v.controller.is_some() {
            self.versions.controller = v.controller;
        }
        if v.dongle.is_some() {
            self.versions.dongle = v.dongle;
        }
        self.current_versions = v;
    }
    fn firmware_label(&self, target: Target) -> String {
        if self.current_versions.boot(target) {
            return format!("{}: bootloader / recovery", target.name());
        }
        match self.versions.get(target) {
            Some(v) => format!(
                "{} FW {}{}",
                target.name(),
                firmware::version_text(v),
                if self.current_versions.get(target).is_none() {
                    " (last read)"
                } else {
                    ""
                }
            ),
            None => format!("{} FW: —", target.name()),
        }
    }
    fn dirty(&self) -> bool {
        self.session
            .as_ref()
            .zip(self.draft.as_ref())
            .is_some_and(|(s, c)| s.config != *c)
    }
    fn save(&mut self, ctx: &egui::Context) {
        let (Some(s), Some(new)) = (&self.session, &self.draft) else {
            return;
        };
        let old = s.config.clone();
        let new = new.clone();
        self.run(ctx, move || {
            let l = Link::connect()?;
            let path = backup::automatic(&l)?;
            l.save_config(&old, &new)
                .map_err(|e| format!("{e}. Backup: {}", path.display()))?;
            load().map(Event::Loaded)
        })
    }
    fn preview(&mut self, ctx: &egui::Context, left: f32, right: f32) {
        let cancel = self.cancel.clone();
        let stop = self.stop.clone();
        let view = self.motion.clone();
        self.run(ctx, move || {
            let l = Link::for_rumble()?;
            view.lock().unwrap().rumble = [left, right];
            l.rumble(left, right, 1000)?;
            let end = Instant::now() + Duration::from_millis(750);
            while Instant::now() < end
                && !cancel.load(Ordering::Relaxed)
                && !stop.load(Ordering::Relaxed)
            {
                thread::sleep(Duration::from_millis(20));
            }
            view.lock().unwrap().rumble = [0.; 2];
            l.rumble(0., 0., 0)?;
            Ok(Event::Message("Rumble preview stopped".into()))
        });
    }
    fn preview_triggers(&mut self, ctx: &egui::Context, left: f32, right: f32, ranges: [u8; 4]) {
        let cancel = self.cancel.clone();
        let stop = self.stop.clone();
        let view = self.motion.clone();
        self.run(ctx, move || {
            let l = Link::for_rumble()?;
            let end = Instant::now() + Duration::from_secs(8);
            let result = (|| {
                while Instant::now() < end
                    && !cancel.load(Ordering::Relaxed)
                    && !stop.load(Ordering::Relaxed)
                {
                    let inputs = view.lock().unwrap().sample.triggers;
                    let motors = std::array::from_fn(|i| {
                        let start = ranges[i * 2] as f32 / 100.;
                        let full = ranges[i * 2 + 1] as f32 / 100.;
                        let amount = if full <= start {
                            f32::from(inputs[i] >= start)
                        } else {
                            ((inputs[i] - start) / (full - start)).clamp(0., 1.)
                        };
                        amount * [left, right][i]
                    });
                    view.lock().unwrap().rumble = motors;
                    l.rumble(motors[0], motors[1], 200)?;
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(())
            })();
            view.lock().unwrap().rumble = [0.; 2];
            let stopped = l.rumble(0., 0., 0);
            result.and(stopped)?;
            Ok(Event::Message("Trigger test stopped".into()))
        });
        if self.busy {
            self.status =
                "Press LT and RT to test their rumble response · stops after 8 seconds".into();
        }
    }
    fn preview_panel(&mut self, ui: &mut egui::Ui, compact: bool) {
        let view = self.motion.lock().unwrap().clone();
        ui.horizontal_wrapped(|ui| {
            ui.heading(view.mode);
            ui.label(view.connection);
        });
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("Change input mode", |ui| self.mode_controls(ui));
            if let Some(b) = self.session.as_ref().and_then(|s| s.info.battery) {
                ui.small(format!("Battery {b}%"));
            }
        });
        if self.session.is_none()
            && !self.busy
            && !view.path.is_empty()
            && self.auto_loaded_path != view.path
        {
            self.auto_loaded_path = view.path.clone();
            self.run(ui.ctx(), refresh);
        }
        let height = if compact {
            (ui.ctx().viewport_rect().height() * 0.24).clamp(140., 220.)
        } else {
            (ui.ctx().viewport_rect().height() * 0.38).clamp(190., 400.)
        };
        let zones = self
            .draft
            .as_ref()
            .filter(|c| c.u32(self.slot * 4) == ENABLED)
            .map(|c| std::array::from_fn(|i| c.0[156 + 8 * self.slot + i].min(128) as f32 / 128.))
            .unwrap_or([17. / 128., 1., 17. / 128., 1.]);
        let action = drawing::controller(
            ui,
            &view,
            self.follow_motion,
            self.theme != 5,
            height,
            zones,
        );
        if let Some((key, section)) = action {
            self.tab = section;
            self.focus_button = if section == 1 { Some(key) } else { None };
            self.scroll_section = Some(section);
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                egui::Id::unique(("native-section", section)),
                true,
            );
            state.set_open(true);
            state.store(ui.ctx());
            ui.ctx().request_repaint();
        }
        if compact {
            let pressed = BUTTONS
                .iter()
                .zip(motion::BUTTON_BITS)
                .filter(|(_, bit)| view.sample.buttons & bit != 0)
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(" + ");
            ui.label(
                egui::RichText::new(if pressed.is_empty() {
                    "Press any button".into()
                } else {
                    pressed
                })
                .color(GREEN),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                for (label, bit) in BUTTONS.iter().zip(motion::BUTTON_BITS) {
                    let down = view.sample.buttons & bit != 0;
                    ui.add(
                        egui::Button::new(egui::RichText::new(*label).size(13.))
                            .sense(egui::Sense::hover())
                            .wrap_mode(egui::TextWrapMode::Extend)
                            .selected(down),
                    );
                }
            });
        }
        let known = motion::BUTTON_BITS.iter().fold(0, |a, b| a | b);
        let extra = view.sample.buttons & !known;
        if extra != 0 {
            ui.colored_label(GREEN, format!("Other physical input: 0x{extra:08x}"));
        }
        ui.add_space(8.);
        ui.columns(2, |columns| {
            for (i, name) in ["LT", "RT"].iter().enumerate() {
                columns[i].add(
                    egui::ProgressBar::new(view.sample.triggers[i])
                        .text(format!("{name} {:.0}%", view.sample.triggers[i] * 100.)),
                );
            }
        });
        if !compact {
            ui.small("Amber: deadzone. Blue: outer range. White dot: raw stick input.");
            ui.small("Paddles and shoulder buttons light up here too. Hardware-only switches and pairing controls may not send an input report.");
        }
    }
    fn settings(&mut self, ui: &mut egui::Ui) {
        for (i, title) in TABS.iter().enumerate().skip(1) {
            let id = egui::Id::unique(("native-section", i));
            let header = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                self.tab == i,
            )
            .show_header(ui, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.), egui::Sense::hover());
                drawing::section_icon(ui.painter(), rect.center(), i);
                ui.add(
                    egui::Label::new(egui::RichText::new(*title).strong().size(18.))
                        .sense(egui::Sense::click()),
                )
            });
            let open = header.is_open();
            let (_, title_response, body) = header.body(|ui| self.section_content(ui, i));
            if title_response.inner.clicked() {
                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    false,
                );
                state.set_open(!open);
                state.store(ui.ctx());
                ui.ctx().request_repaint();
            }
            if self.scroll_section == Some(i) {
                title_response.response.scroll_to_me(Some(egui::Align::Min));
                self.scroll_section = None;
            }
            if i == 4 {
                self.motion_render
                    .store(self.follow_motion || body.is_some(), Ordering::Relaxed);
            }
            ui.separator();
        }
    }
    fn section_content(&mut self, ui: &mut egui::Ui, i: usize) {
        ui.add_space(8.);
        let dirty = self.dirty();
        let mut c = self.draft.take();
        let enabled = c.as_ref().is_some_and(|c| c.u32(self.slot * 4) == ENABLED);
        if (1..=5).contains(&i)
            && ui
                .add_enabled(enabled && !self.busy, egui::Button::new("Reset defaults"))
                .clicked()
            && let Some(c) = &mut c
            && let Err(e) = c.reset_section(self.slot, i)
        {
            self.status = e;
        }
        if [1, 2].contains(&i)
            && !enabled
            && let Some(profile) = &mut c
        {
            ui.label("Enable this profile to customize its settings.");
            if ui.button("Enable profile").clicked() {
                profile.enable_slot(self.slot);
                self.name = profile.name(self.slot);
            }
        } else {
            match i {
                1 => {
                    if let Some(c) = &mut c {
                        self.mappings(ui, c);
                    }
                }
                2 => {
                    if let Some(c) = &mut c {
                        self.sticks(ui, c);
                    }
                }
                3 => self.rumble(ui, if enabled { c.as_mut() } else { None }),
                4 => self.motion(ui, c.as_mut()),
                5 => {
                    if let Some(c) = &mut c {
                        self.lighting(ui, c);
                    }
                }
                6 => self.macros(ui, dirty),
                _ => self.device(ui),
            }
        }
        self.draft = c;
        if self.session.is_none() && [1, 2, 5].contains(&i) {
            ui.label(
                "Connect USB to edit saved settings. Live input and rumble work on the receiver.",
            );
        }
        ui.add_space(8.);
    }
    fn mappings(&mut self, ui: &mut egui::Ui, c: &mut Config) {
        let pairs = if ui.available_width() > 700. { 2 } else { 1 };
        egui::Grid::new("mapping")
            .num_columns(pairs * 2)
            .spacing([24., 10.])
            .show(ui, |ui| {
                for (i, name) in BUTTONS.iter().enumerate() {
                    let focused = self.focus_button == Some(*name);
                    let label = ui.label(egui::RichText::new(*name).color(if focused {
                        GREEN
                    } else {
                        ui.visuals().text_color()
                    }));
                    if focused {
                        label.scroll_to_me(Some(egui::Align::Center));
                        self.focus_button = None;
                    }
                    let o = 228 + 92 * self.slot + 4 * i;
                    let mut value = c.u32(o);
                    egui::ComboBox::from_id_salt(("key", i))
                        .selected_text(config::key_name(value))
                        .show_ui(ui, |ui| {
                            for (name, key) in KEYS {
                                ui.selectable_value(&mut value, key, name);
                            }
                        });
                    c.set32(o, value);
                    if i % pairs == pairs - 1 {
                        ui.end_row();
                    }
                }
            });
    }
    fn range(ui: &mut egui::Ui, c: &mut Config, o: usize, lo: u8, hi: u8, name: &str) {
        let mut v = c.0[o].clamp(lo, hi);
        ui.label(name);
        if ui.add(egui::Slider::new(&mut v, lo..=hi)).changed() {
            c.0[o] = v;
        }
    }
    fn flag(ui: &mut egui::Ui, c: &mut Config, o: usize, mask: u32, name: &str) {
        let mut bits = c.u32(o);
        let mut on = bits & mask != 0;
        if ui.checkbox(&mut on, name).changed() {
            bits = if on { bits | mask } else { bits & !mask };
            c.set32(o, bits);
        }
    }
    fn sticks(&mut self, ui: &mut egui::Ui, c: &mut Config) {
        for (i, name) in [
            "Left dead zone",
            "Left outer range",
            "Right dead zone",
            "Right outer range",
        ]
        .iter()
        .enumerate()
        {
            Self::range(ui, c, 156 + 8 * self.slot + i, 0, 128, name);
        }
        ui.separator();
        ui.heading("Triggers");
        for (i, name) in ["LT start", "LT full press", "RT start", "RT full press"]
            .iter()
            .enumerate()
        {
            Self::range(ui, c, 180 + 8 * self.slot + i, 0, 255, name);
        }
        ui.separator();
        for (mask, name) in [
            (1, "Invert left X"),
            (2, "Invert left Y"),
            (4, "Invert right X"),
            (8, "Invert right Y"),
            (16, "Swap sticks"),
            (128, "Swap triggers"),
            (256, "Swap left stick and D-pad"),
        ] {
            Self::flag(ui, c, 204 + 8 * self.slot, mask, name);
        }
    }
    fn rumble(&mut self, ui: &mut egui::Ui, mut c: Option<&mut Config>) {
        let o = 120 + 12 * self.slot;
        let mut left = c
            .as_ref()
            .map(|c| c.float(o))
            .unwrap_or(self.preview_levels[0]);
        let mut right = c
            .as_ref()
            .map(|c| c.float(o + 4))
            .unwrap_or(self.preview_levels[1]);
        ui.label("Left motor");
        if ui
            .add(
                egui::Slider::new(&mut left, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.))
                    .custom_parser(|s| {
                        s.trim_end_matches('%')
                            .parse::<f64>()
                            .ok()
                            .map(|n| n / 100.)
                    }),
            )
            .changed()
        {
            if let Some(c) = &mut c {
                c.set_float(o, left);
            }
            self.preview_levels[0] = left;
        }
        ui.label("Right motor");
        if ui
            .add(
                egui::Slider::new(&mut right, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.))
                    .custom_parser(|s| {
                        s.trim_end_matches('%')
                            .parse::<f64>()
                            .ok()
                            .map(|n| n / 100.)
                    }),
            )
            .changed()
        {
            if let Some(c) = &mut c {
                c.set_float(o + 4, right);
            }
            self.preview_levels[1] = right;
        }
        ui.add_space(16.);
        ui.horizontal_wrapped(|ui| {
            let ctx = ui.ctx().clone();
            ui.add_enabled_ui(!self.busy, |ui| {
                if ui.button("Preview left").clicked() {
                    self.preview(&ctx, left, 0.);
                }
                if ui.button("Preview right").clicked() {
                    self.preview(&ctx, 0., right);
                }
                if ui.button("Preview both").clicked() {
                    self.preview(&ctx, left, right);
                }
            });
            if ui.button("Stop preview").clicked() {
                self.cancel.store(true, Ordering::Relaxed);
            }
        });
        ui.label("Each preview lasts 0.75 seconds and sends an explicit stop.");
        let ranges = c
            .as_ref()
            .map(|c| {
                let o = 1152 + 8 * self.slot;
                std::array::from_fn(|i| c.0[o + i].clamp(1, 100))
            })
            .unwrap_or([1, 100, 1, 100]);
        if ui
            .add_enabled(!self.busy, egui::Button::new("Test LT / RT response (8 s)"))
            .clicked()
        {
            self.preview_triggers(ui.ctx(), left, right, ranges);
        }
        ui.small("Pull LT/RT during the test. Trigger effects drive this controller's grip motors; it has no separately addressable trigger motors.");

        ui.add_space(16.);
        let Some(c) = c else {
            ui.small("Preview only · connect USB to edit saved profile strengths.");
            return;
        };
        Self::flag(ui, c, 204 + 8 * self.slot, 0x8000, "Enable trigger rumble");
        for (i, name) in [
            "LT rumble start",
            "LT rumble end",
            "RT rumble start",
            "RT rumble end",
        ]
        .iter()
        .enumerate()
        {
            Self::range(ui, c, 1152 + 8 * self.slot + i, 1, 100, name);
        }
    }
    fn motion(&mut self, ui: &mut egui::Ui, c: Option<&mut Config>) {
        let view = self.motion.lock().unwrap().clone();
        ui.label(&view.status);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(view.has_imu, egui::Button::new("Calibrate / recenter"))
                .clicked()
            {
                self.motion.lock().unwrap().calibrate = true;
            }
            ui.label(&view.calibration);
            ui.label(format!("{:.0} reports/s", view.hz));
        });
        ui.checkbox(
            &mut self.follow_motion,
            "Rotate the controller preview with motion",
        );
        ui.columns(3, |columns| {
            for (i, name) in ["Pitch", "Yaw", "Roll"].iter().enumerate() {
                columns[i].label(
                    egui::RichText::new(*name)
                        .strong()
                        .color([GREEN, Color32::LIGHT_BLUE, Color32::LIGHT_RED][i]),
                );
                columns[i].label(egui::RichText::new(format!("{:+.1}°", view.angles[i])).size(24.));
                columns[i].label(format!("{:+.1} °/s", view.sample.gyro[i]));
            }
        });
        ui.add_space(8.);
        ui.label(format!(
            "Acceleration  {:+.2} / {:+.2} / {:+.2} g   ·   LT {:.0}% / RT {:.0}%",
            view.sample.accel[0],
            view.sample.accel[1],
            view.sample.accel[2],
            view.sample.triggers[0] * 100.,
            view.sample.triggers[1] * 100.
        ));
        ui.small("Drift is corrected automatically while still. Recenter sets the current pose as neutral. Yaw is relative: this six-axis controller has no compass.");
        self.history.push(view.sample.gyro);
        if self.history.len() > 120 {
            self.history.remove(0);
        }
        let (r, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 90.), egui::Sense::hover());
        let p = ui.painter_at(r);
        p.rect_filled(r, 4., Color32::from_gray(20));
        for axis in 0..3 {
            let line: Vec<Pos2> = self
                .history
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    Pos2::new(
                        r.left() + i as f32 / 120. * r.width(),
                        r.center().y - v[axis].clamp(-500., 500.) / 500. * r.height() / 2.,
                    )
                })
                .collect();
            p.add(egui::Shape::line(
                line,
                Stroke::new(1.4, [GREEN, Color32::LIGHT_BLUE, Color32::LIGHT_RED][axis]),
            ));
        }
        if let Some(c) = c.filter(|c| c.u32(self.slot * 4) == ENABLED) {
            ui.separator();
            ui.heading("Gyro mapping");
            let o = 1172 + 12 * self.slot;
            let mut enabled = c.u32(o) == ENABLED;
            if ui
                .checkbox(&mut enabled, "Enable gyro-to-stick mapping")
                .changed()
            {
                c.set32(o, if enabled { ENABLED } else { 0x20190000 });
            }
            egui::ComboBox::from_id_salt("gyroout")
                .selected_text(match c.0[o + 11] {
                    1 => "Right stick",
                    2 => "Left stick",
                    _ => "Off",
                })
                .show_ui(ui, |ui| {
                    for (v, n) in [(0, "Off"), (1, "Right stick"), (2, "Left stick")] {
                        ui.selectable_value(&mut c.0[o + 11], v, n);
                    }
                });
            Self::range(ui, c, o + 9, 1, 10, "Sensitivity");
            Self::range(ui, c, o + 10, 0, 100, "Dead-zone compensation");
            let mut key = c.u32(o + 4);
            egui::ComboBox::from_id_salt("gyrokey")
                .selected_text(config::key_name(key))
                .show_ui(ui, |ui| {
                    for (name, value) in KEYS
                        .into_iter()
                        .filter(|(n, _)| ["LT", "RT", "LB", "RB", "L3", "R3"].contains(n))
                    {
                        ui.selectable_value(&mut key, value, name);
                    }
                });
            c.set32(o + 4, key);
            egui::ComboBox::from_id_salt("gyroactivation")
                .selected_text(if c.0[o + 8] == 2 { "Toggle" } else { "Hold" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut c.0[o + 8], 1, "Hold");
                    ui.selectable_value(&mut c.0[o + 8], 2, "Toggle");
                });
        }
    }
    fn color(ui: &mut egui::Ui, c: &mut Config, o: usize, label: &str) {
        let value = c.u32(o);
        let mut rgb = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
        ui.horizontal_wrapped(|ui| {
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                c.set32(
                    o,
                    ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32,
                );
            }
            ui.label(label);
        });
    }
    fn lighting(&mut self, ui: &mut egui::Ui, c: &mut Config) {
        let before = self.theme;
        ui.horizontal_wrapped(|ui| {
            if self.busy {
                ui.disable();
            }
            for (v, n) in [(2, "Dynamic"), (3, "Fire ring"), (4, "Custom"), (5, "Off")] {
                ui.selectable_value(&mut self.theme, v, n);
            }
        });
        if before != self.theme {
            let t = self.theme;
            let edition = self.session.as_ref().map(|s| s.info.edition).unwrap_or(0);
            self.run(ui.ctx(), move || {
                let l = Link::connect()?;
                backup::automatic(&l)?;
                l.set_theme(t, edition)?;
                Ok(Event::Lighting(t))
            });
        }
        let s = self.slot;
        if c.u32(s * 4) != ENABLED {
            ui.label("The theme applies immediately. Enable a profile below to customize colours.");
            return;
        }
        Self::color(ui, c, 1212 + 12 * s, "Dynamic foreground");
        Self::color(ui, c, 1216 + 12 * s, "Dynamic background");
        Self::color(ui, c, 1248 + 16 * s, "Fire ring foreground");
        Self::color(ui, c, 1252 + 16 * s, "Fire ring background");
        Self::range(ui, c, 1256 + 16 * s, 1, 15, "Fire ring speed");
        ui.label("Custom LEDs");
        egui::Grid::new("leds").num_columns(6).show(ui, |ui| {
            for i in 0..24 {
                let value = c.u32(1296 + 100 * s + 4 * i);
                let mut rgb = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    c.set32(
                        1296 + 100 * s + 4 * i,
                        ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32,
                    );
                }
                if i % 6 == 5 {
                    ui.end_row();
                }
            }
        });
    }
    fn macros(&mut self, ui: &mut egui::Ui, dirty: bool) {
        if ui.button("Reset macro editor").clicked() {
            self.macro_data = Macro::default();
        }
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("macroindex")
                .selected_text(format!("Macro {}", self.macro_index + 1))
                .show_ui(ui, |ui| {
                    for i in 0..4 {
                        ui.selectable_value(&mut self.macro_index, i, format!("Macro {}", i + 1));
                    }
                });
            if ui
                .add_enabled(!self.busy, egui::Button::new("Load from controller"))
                .clicked()
            {
                let (s, i) = (self.slot, self.macro_index);
                self.run(ui.ctx(), move || {
                    let l = Link::connect()?;
                    let c = l.read_config()?;
                    macros::read(&l, &c, s, i).map(Event::Macro)
                });
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.macro_data.name);
            ui.label("Trigger");
            egui::ComboBox::from_id_salt("macrotrigger")
                .selected_text(config::key_name(self.macro_data.trigger))
                .show_ui(ui, |ui| {
                    for (n, k) in KEYS
                        .into_iter()
                        .filter(|(_, k)| *k == 0 || macros::TRIGGERS.contains(k))
                    {
                        ui.selectable_value(&mut self.macro_data.trigger, k, n);
                    }
                });
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Repeats");
            ui.add(egui::DragValue::new(&mut self.macro_data.cycles).range(1..=1000));
            ui.label("Between repeats (ms)");
            ui.add(egui::DragValue::new(&mut self.macro_data.interval_ms).range(0..=60000));
        });
        ui.add_space(8.);
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("steps")
            .max_height((ui.ctx().viewport_rect().height() * 0.4).max(180.))
            .show(ui, |ui| {
                for (i, step) in self.macro_data.steps.iter_mut().enumerate() {
                    ui.group(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("{}", i + 1));
                            ui.add(
                                egui::DragValue::new(&mut step.duration_ms)
                                    .range(1..=65535)
                                    .suffix(" ms"),
                            );
                            egui::ComboBox::from_id_salt(("stepkeys", i))
                                .width(180.)
                                .truncate()
                                .selected_text(if step.keys == 0 {
                                    "Release / wait".into()
                                } else {
                                    KEYS.iter()
                                        .filter(|(_, k)| {
                                            *k > 0 && *k <= 0xffff && step.keys & *k as u16 != 0
                                        })
                                        .map(|(n, _)| *n)
                                        .collect::<Vec<_>>()
                                        .join("+")
                                })
                                .show_ui(ui, |ui| {
                                    for (n, k) in
                                        KEYS.into_iter().filter(|(_, k)| *k > 0 && *k <= 0xffff)
                                    {
                                        let mut on = step.keys & k as u16 != 0;
                                        if ui.checkbox(&mut on, n).changed() {
                                            if on {
                                                step.keys |= k as u16
                                            } else {
                                                step.keys &= !(k as u16)
                                            }
                                        }
                                    }
                                });
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            for (j, n) in ["LT", "RT"].iter().enumerate() {
                                ui.label(*n);
                                ui.add(egui::DragValue::new(&mut step.triggers[j]).range(0..=255));
                            }
                            for (n, pair) in [
                                ("Left stick", &mut step.left),
                                ("Right stick", &mut step.right),
                            ] {
                                ui.label(n);
                                for v in pair {
                                    let mut percent = (*v as f32 - 127.) / 1.28;
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut percent)
                                                .range(-100.0..=100.0)
                                                .suffix("%"),
                                        )
                                        .changed()
                                    {
                                        *v = (percent * 1.28 + 127.).clamp(0., 255.) as u8;
                                    }
                                }
                            }
                        });
                    });
                }
            });
        if let Some(i) = remove {
            self.macro_data.steps.remove(i);
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.macro_data.steps.len() < 200,
                    egui::Button::new("Add step"),
                )
                .clicked()
            {
                self.macro_data.steps.push(Step::default());
            }
            if ui.button("Clear steps").clicked() {
                self.macro_data.steps.clear();
            }
            ui.label(format!("{} / 200 steps", self.macro_data.steps.len()));
        });
        if ui
            .add_enabled(
                !self.busy && self.session.is_some() && !dirty,
                egui::Button::new("Save macro to controller"),
            )
            .clicked()
        {
            let (s, i, m) = (self.slot, self.macro_index, self.macro_data.clone());
            self.run(ui.ctx(), move || {
                macros::validate(&m)?;
                let l = Link::connect()?;
                let path = backup::automatic(&l)?;
                let old = l.read_config()?;
                l.report(false)?;
                let result = macros::write_steps(&l, &old, s, i, &m);
                let resume = l.report(true);
                result
                    .and(resume)
                    .map_err(|e| format!("{e}. Backup: {}", path.display()))?;
                let new = macros::metadata(&old, s, i, &m)?;
                l.save_config(&old, &new)?;
                load().map(Event::Loaded)
            });
        }
        if dirty {
            ui.label("Save your profile changes before writing a macro.");
        }
        ui.small("Each step holds its selected inputs for the duration. Add a release step to release buttons. An automatic backup is saved before writing.");
    }
    fn mode_controls(&mut self, ui: &mut egui::Ui) {
        ui.set_max_width(440.);
        ui.strong("Choose the mode while powering on");
        ui.label(
            "Unplug the controller's USB cable, set its rear switch to 2.4G, then power it off.",
        );
        for (mode, key) in [("DInput", "B"), ("XInput", "X"), ("Switch", "Y")] {
            ui.horizontal_wrapped(|ui| {
                ui.strong(mode);
                ui.label(format!("Hold {key}, then press Home to turn it on."));
            });
        }
        ui.small("The detected mode updates on the main screen. The controller's firmware requires these physical button combinations.");
    }
    fn device(&mut self, ui: &mut egui::Ui) {
        ui.heading("Firmware updates");
        ui.small("Experimental: downloads and bootloader entry/reset are tested; actual flashing is not yet tested.");
        ui.small("Controller: connect its USB cable. Dongle: use XInput, or turn the controller off and reconnect the receiver.");
        ui.horizontal_wrapped(|ui| {
            let old = self.target;
            egui::ComboBox::from_id_salt("target")
                .selected_text(self.target.name())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.target, Target::Controller, "Controller");
                    ui.selectable_value(&mut self.target, Target::Receiver, "Dongle");
                });
            if old != self.target {
                self.releases.clear();
            }
            if ui
                .add_enabled(!self.busy, egui::Button::new("Check official releases"))
                .clicked()
            {
                let target = self.target;
                self.run(ui.ctx(), move || {
                    Ok(Event::Releases(
                        firmware::releases(target)?,
                        firmware::versions()?,
                    ))
                });
            }
        });
        if !self.releases.is_empty() {
            egui::ComboBox::from_id_salt("release")
                .selected_text(&self.releases[self.release_index].version)
                .show_ui(ui, |ui| {
                    for (i, r) in self.releases.iter().enumerate() {
                        ui.selectable_value(&mut self.release_index, i, &r.version);
                    }
                });
            ui.label(self.firmware_label(self.target));
            let wanted = firmware::version_number(&self.releases[self.release_index].version);
            let allowed =
                wanted.is_some_and(|v| self.current_versions.check_update(self.target, v).is_ok());
            if wanted.is_some_and(|v| self.current_versions.get(self.target) == Some(v)) {
                ui.colored_label(GREEN, "This version is already installed");
            }
            ui.label(&self.releases[self.release_index].readme_en);
            ui.checkbox(&mut self.confirm_flash,"The selected device is directly connected by USB; keep it connected until verification finishes");
            if ui
                .add_enabled(
                    self.confirm_flash && !self.busy && allowed,
                    egui::Button::new(format!("Update {}", self.target.name())),
                )
                .clicked()
            {
                self.flashing = true;
                let target = self.target;
                let release = self.releases[self.release_index].clone();
                let progress = self.progress.clone();
                *progress.lock().unwrap() = 0.;
                self.run(ui.ctx(), move || {
                    let image = firmware::download(&release, target)?;
                    let state = firmware::versions()?;
                    state.check_update(target, image.version)?;
                    if target == Target::Controller && !state.boot(target) {
                        backup::automatic(&Link::connect()?)?;
                    }
                    let link = firmware::boot(target)?;
                    firmware::flash(&link, &image, |p| *progress.lock().unwrap() = p)?;
                    drop(link);
                    firmware::verify(target, image.version)?;
                    Ok(Event::Message(format!(
                        "{} firmware {} installed and verified",
                        target.name(),
                        release.version
                    )))
                });
            }
        }
        if self.busy {
            ui.add(egui::ProgressBar::new(*self.progress.lock().unwrap()).show_percentage());
        }
        ui.small("The controller and dongle use different images. Native flashing validates the target, uses the vendor's bootloader protocol, and verifies the version after reconnecting.");
        ui.separator();
        ui.heading("Pair the wireless receiver");
        ui.label("1. Disconnect the controller's cable and set its rear switch to 2.4G.\n2. Turn the controller off; connect the receiver.\n3. Start detection below, then turn on the controller and hold Pair for 3 seconds.");
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.busy, egui::Button::new("Detect pairing (45 s)"))
                .clicked()
            {
                let cancel = self.cancel.clone();
                self.run(ui.ctx(), move || {
                    device::wait_pair(&cancel)?;
                    Ok(Event::Message(
                        "Controller connected to the receiver".into(),
                    ))
                });
            }
            if ui.button("Cancel detection").clicked() {
                self.cancel.store(true, Ordering::Relaxed);
            }
        });
        ui.separator();
        ui.heading("Backup & restore");
        ui.label(format!(
            "Automatic backups: {}",
            backup::directory().display()
        ));
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.busy, egui::Button::new("Back up now"))
                .clicked()
            {
                self.run(ui.ctx(), || {
                    let path = backup::automatic(&Link::connect()?)?;
                    Ok(Event::Message(format!("Backup saved: {}", path.display())))
                });
            }
            ui.label("Restore file");
            ui.add(
                egui::TextEdit::singleline(&mut self.backup_path)
                    .desired_width(ui.available_width().min(420.)),
            );
            if ui
                .add_enabled(
                    !self.busy && !self.backup_path.is_empty(),
                    egui::Button::new("Restore"),
                )
                .clicked()
            {
                let path = self.backup_path.clone();
                self.run(ui.ctx(), move || {
                    backup::restore(&Link::connect()?, Path::new(&path))?;
                    load().map(Event::Loaded)
                });
            }
        });
    }
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(feature = "screenshots")]
        if let Some(path) = std::env::var_os("U2_SCREENSHOT_TO") {
            if ui.ctx().cumulative_pass_nr() == 4 {
                let ctx = ui.ctx().clone();
                ui.ctx().request_screenshot(move |image| {
                    let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                    image::save_buffer(
                        &path,
                        &bytes,
                        image.size[0] as u32,
                        image.size[1] as u32,
                        image::ColorType::Rgba8,
                    )
                    .expect("Save screenshot");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                });
            }
            ui.ctx().request_repaint();
        }
        self.events();
        if self.flashing && ui.input(|i| i.viewport().close_requested()) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.status = "Wait for the firmware update to finish before closing".into();
        }
        let generation = self.motion.lock().unwrap().generation;
        if generation != self.info_generation && !self.busy && !self.dirty() {
            self.info_generation = generation;
            self.run(ui.ctx(), refresh);
        }
        let visible = !ui.input(|i| i.viewport().minimized.unwrap_or(false));
        self.motion_active
            .store(visible && !self.flashing, Ordering::Relaxed);
        if visible && (self.busy || self.motion.lock().unwrap().mode != "Disconnected") {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(if self.busy { 33 } else { 100 }));
        }
        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(8.);
            ui.horizontal_wrapped(|ui| {
                ui.heading("Ultimate 2");
                ui.small("Wireless controller");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("Reconnect"))
                        .clicked()
                    {
                        self.run(ui.ctx(), refresh);
                    }
                });
            });
        });
        egui::Panel::top("device-info").show(ui,|ui| {
            ui.horizontal_wrapped(|ui|{
                ui.small(self.firmware_label(Target::Controller));
                ui.small(self.firmware_label(Target::Receiver));
                ui.small("Polling: up to 1000 Hz · USB / 2.4G").on_hover_text("No adjustable polling-rate command has been verified. Motion displays the measured sensor report rate.");
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(4.);
            ui.horizontal_wrapped(|ui| {
                if self.busy {
                    ui.spinner();
                }
                ui.small(&self.status);
            });
        });
        if self.draft.is_some() {
            egui::Panel::bottom("profile").show(ui, |ui| {
                ui.add_space(8.);
                ui.horizontal_wrapped(|ui| {
                    if self.busy {
                        ui.disable();
                    }
                    let old = self.slot;
                    egui::ComboBox::from_id_salt("profile")
                        .selected_text(format!("Profile {}", self.slot + 1))
                        .show_ui(ui, |ui| {
                            for i in 0..3 {
                                ui.selectable_value(
                                    &mut self.slot,
                                    i,
                                    format!("Profile {}", i + 1),
                                );
                            }
                        });
                    if old != self.slot {
                        self.name = self.draft.as_ref().unwrap().name(self.slot);
                    }
                    if let Some(c) = &mut self.draft {
                        let mut on = c.u32(self.slot * 4) == ENABLED;
                        if ui.checkbox(&mut on, "Enabled").changed() {
                            if on {
                                c.enable_slot(self.slot);
                                self.name = c.name(self.slot);
                            } else {
                                c.set32(self.slot * 4, 0x20190000);
                            }
                        }
                        if ui
                            .add_enabled(
                                on,
                                egui::TextEdit::singleline(&mut self.name)
                                    .desired_width(150.)
                                    .hint_text("Profile name"),
                            )
                            .changed()
                        {
                            c.set_name(self.slot, &self.name);
                        }
                    }
                    let dirty = self.dirty();
                    if ui
                        .add_enabled(
                            dirty && !self.busy,
                            egui::Button::new("Save profile").fill(Color32::from_rgb(43, 83, 70)),
                        )
                        .clicked()
                    {
                        self.save(ui.ctx());
                    }
                    if dirty {
                        ui.small("Unsaved changes");
                    }
                });
                ui.add_space(8.);
            });
        }
        let wide = ui.available_width() >= 1040.;
        if wide {
            egui::Panel::left("preview")
                .resizable(true)
                .default_size(ui.available_width() * 0.46)
                .size_range(340.0..=(ui.available_width() - 420.).min(800.))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("livepreview")
                        .show(ui, |ui| self.preview_panel(ui, false));
                });
        }
        if !wide {
            let available = ui.available_height();
            egui::Panel::top("compact-preview")
                .resizable(true)
                .default_size((available * 0.55).clamp(180., 380.))
                .size_range(150.0..=(available - 150.).max(150.))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("compact-live")
                        .show(ui, |ui| self.preview_panel(ui, true));
                });
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(Color32::from_rgb(23, 28, 35))
                    .inner_margin(20),
            )
            .show(ui, |ui| {
                ui.spacing_mut().slider_width = (ui.available_width() - 90.).clamp(100., 500.);
                egui::ScrollArea::vertical()
                    .id_salt("settings-scroll")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .show(ui, |ui| {
                        self.settings(ui);
                    });
            });
    }
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [23. / 255., 28. / 255., 35. / 255., 1.]
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop.store(true, Ordering::Relaxed);
        self.cancel.store(true, Ordering::Relaxed);
        if let Ok(ns) = device::nodes() {
            for n in ns.iter().filter(|n| n.motion && !n.config) {
                if let Ok(l) = Link::open(n) {
                    let _ = l.rumble(0., 0., 0);
                }
            }
        }
    }
}
