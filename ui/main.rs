mod bridge;
mod history;
mod hud;
mod preferences;
mod recording;
mod view;
use fire_ui::*;
use fire_ui_native::{run_with, WindowOptions};
use fire_ui_widgets::*;
use history::{HistoryCommand, HistoryRow};
use preferences::{PreferenceCommand, Preferences};
use recording::{Recording, RecordingCommand};
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Arc, time::Duration};

const ACTIONS: [&str; 13] = [
    "Pause dictation",
    "Copy text",
    "Cancel dictation",
    "Save changes",
    "Test microphone",
    "Capture hotkey",
    "Refresh",
    "Copy report",
    "Dismiss",
    "Previous",
    "Next",
    "Debug: off",
    "Open recordings folder",
];
const FIELDS: [(&str, &str, &str); 11] = [
    ("Microphone", "audio", "pipewire_node"),
    ("Model", "transcription", "model"),
    ("Compute type", "transcription", "compute_type"),
    ("Accuracy / beam size", "transcription", "beam_size"),
    ("Remove silence", "transcription", "vad_filter"),
    ("Normalize audio", "audio", "preprocess"),
    ("Text output", "output", "method"),
    ("Space after text", "output", "add_space"),
    ("Notifications", "notifications", "enabled"),
    ("Audio feedback", "notifications", "audio_feedback"),
    ("Floating indicator", "ui", "cursor_indicator"),
];
type Control = Button<Label>;

#[derive(Clone)]
enum Output {
    Start,
    Request(Arc<Value>),
}
impl Data for Output {
    fn bytes(&self) -> usize {
        match self {
            Self::Start => 0,
            Self::Request(v) => v.to_string().len(),
        }
    }
}
#[derive(Clone)]
enum Command {
    Backend(Arc<Value>),
    Page(usize),
    Action(usize),
    Setting(usize, Value),
    History(usize, usize),
    Search(EditorOutput),
    Transcript(EditorOutput),
    Language(EditorOutput),
}
impl Data for Command {
    fn bytes(&self) -> usize {
        match self {
            Self::Backend(v) => v.to_string().len(),
            Self::Setting(_, v) => v.to_string().len(),
            Self::Search(o) | Self::Transcript(o) | Self::Language(o) => o.bytes(),
            _ => 32,
        }
    }
}

struct Level(f32, bool);
impl Data for Level {
    fn bytes(&self) -> usize {
        8
    }
}
struct Wave {
    levels: VecDeque<f32>,
    active: bool,
}
impl Widget for Wave {
    type Command = Level;
    type Output = std::convert::Infallible;
    fn update(&mut self, cx: &mut Update<'_, Self>, Level(level, active): Self::Command) {
        self.active = active;
        if !active {
            self.levels.clear();
        } else {
            self.levels.push_back(level.clamp(0., 1.));
            if self.levels.len() > 48 {
                self.levels.pop_front();
            }
        }
        cx.repaint();
    }
    fn layout(&mut self, _: &mut Layout<'_>, c: Constraints) -> Metrics {
        Metrics::new(c.constrain(Size::new(c.max.width, theme().scale.space(6.))))
    }
    fn paint(&self, cx: &mut Paint<'_>) {
        let w = cx.bounds.width;
        let h = cx.bounds.height;
        for i in 0..48 {
            let value = self.levels.get(i).copied().unwrap_or(0.);
            let bar = if self.active {
                4. + value * (h - 12.)
            } else {
                3.
            };
            cx.painter.rect(
                Rect::new(
                    i as f32 * w / 48.,
                    (h - bar) / 2.,
                    (w / 48. - 4.).max(2.),
                    bar,
                ),
                2.,
                if self.active {
                    theme().color.success
                } else {
                    theme().color.faint
                }
                .into(),
            );
        }
    }
}
struct Desktop {
    nav: Child<Tabs>,
    brand: Child<Label>,
    title: Child<Label>,
    subtitle: Child<Label>,
    section: Child<Label>,
    hotkey: Child<Label>,
    feedback: Child<Label>,
    spacer: Child<Spacer>,
    preferences: Child<Scroll<Preferences>>,
    recording: Child<Surface<Recording>>,
    meter: Child<Progress>,
    actions: Vec<Child<Control>>,
    rows: Vec<Child<Surface<HistoryRow>>>,
    search: Child<Editor>,
    transcript: Child<Editor>,
    page: usize,
    state: Value,
    config: Value,
    microphones: Vec<Value>,
    history: Vec<Value>,
    history_page: usize,
    search_text: String,
    transcript_text: String,
    diagnostics: String,
    notice: String,
    status: String,
    connected: bool,
    dirty: bool,
    saving: bool,
    closing: bool,
    loaded: bool,
    test_active: bool,
    hotkey_waiting: bool,
    search_timer: Timer,
    log_timer: Timer,
    visible_rows: usize,
}
fn control(label: &str) -> Element<Control> {
    Button::new(Element::leaf(Label::new(label)), label)
}
impl Desktop {
    fn new() -> Element<Self> {
        Element::build(|c| Self {
            nav: c.connect(
                Tabs::new(["Dictate", "History", "Settings", "Status"]),
                |i| Command::Page(*i),
            ),
            brand: c.add(Element::leaf(Label::toned(
                "Voice Dictation",
                TextRole::Small,
                ColorRole::Muted,
            ))),
            title: c.add(Element::leaf(Label::styled("Dictation", TextRole::Title))),
            subtitle: c.add(Element::leaf(
                Label::toned("", TextRole::Small, ColorRole::Muted).wrap(),
            )),
            section: c.add(Element::leaf(Label::styled(
                "Latest transcript",
                TextRole::Heading,
            ))),
            hotkey: c.add(Element::leaf(
                Label::toned("", TextRole::Small, ColorRole::Muted).wrap(),
            )),
            feedback: c.add(Element::leaf(
                Label::toned("", TextRole::Small, ColorRole::Muted).wrap(),
            )),
            spacer: c.add(Spacer::flexible()),
            preferences: c.connect(Scroll::new(Preferences::new()), |command| command.clone()),
            recording: c.add(Surface::new(Recording::new())),
            meter: c.discard(Progress::new("Microphone level", 0.)),
            actions: ACTIONS
                .iter()
                .enumerate()
                .map(|(i, l)| c.connect(control(l), move |_| Command::Action(i)))
                .collect(),
            rows: (0..8)
                .map(|i| {
                    c.connect(Surface::new(HistoryRow::new()), move |action| {
                        Command::History(i, *action)
                    })
                })
                .collect(),
            search: c.connect(
                Element::leaf(
                    Editor::field("")
                        .placeholder("Search dictations")
                        .caret_blink(false),
                ),
                |e| Command::Search(e.clone()),
            ),
            transcript: c.connect(
                Element::leaf(
                    Editor::new("")
                        .placeholder("Your words will appear here.")
                        .caret_blink(false)
                        .chrome(true),
                ),
                |e| Command::Transcript(e.clone()),
            ),
            page: 0,
            state: json!({}),
            config: json!({}),
            microphones: vec![],
            history: vec![],
            history_page: 0,
            search_text: String::new(),
            transcript_text: String::new(),
            diagnostics: String::new(),
            notice: String::new(),
            status: "Connecting to the speech service".into(),
            connected: false,
            dirty: false,
            saving: false,
            closing: false,
            loaded: false,
            test_active: false,
            hotkey_waiting: false,
            search_timer: Timer::new(),
            log_timer: Timer::new(),
            visible_rows: 8,
        })
    }
    fn request(&mut self, cx: &mut Update<'_, Self>, v: Value) {
        if cx.emit(Output::Request(Arc::new(v))).is_err() {
            self.notice = "Too many pending actions. Try again.".into();
            cx.relayout();
        }
    }
    fn button_text(cx: &mut Update<'_, Self>, child: Child<Control>, text: String) {
        let _ = cx.send(child, ButtonCommand::Content(text.clone()));
        let _ = cx.send(child, ButtonCommand::Label(text));
    }
    fn set_page(&mut self, cx: &mut Update<'_, Self>, page: usize) {
        if self.page == page {
            return;
        }
        self.page = page;
        let _ = cx.send(self.nav, TabsCommand(page));
        if matches!(
            self.notice.as_str(),
            "Copied to clipboard." | "Report copied." | "Settings applied."
        ) {
            self.notice.clear();
        }
        if page == 1 {
            self.diagnostics.clear();
            let _ = cx.send(self.transcript, Edit::Set(String::new()));
        }
        cx.cancel_timer(self.log_timer);
        if page == 1 {
            self.request(cx, json!({"cmd":"history","search":self.search_text}));
        }
        if page == 3 {
            self.request(cx, json!({"cmd":"diagnostics"}));
        }
        if page == 0 {
            let _ = cx.send(self.transcript, Edit::Set(self.transcript_text.clone()));
        }
        self.refresh(cx);
    }
    fn refresh(&mut self, cx: &mut Update<'_, Self>) {
        let listening = self.state["listening"].as_bool().unwrap_or(false);
        let debug = self.state["log_level"]
            .as_str()
            .or(self.config["logging"]["level"].as_str())
            == Some("DEBUG");
        Self::button_text(
            cx,
            self.actions[11],
            if debug { "Debug: on" } else { "Debug: off" }.into(),
        );
        Self::button_text(
            cx,
            self.actions[0],
            if listening {
                "Pause dictation"
            } else {
                "Resume dictation"
            }
            .into(),
        );
        Self::button_text(
            cx,
            self.actions[3],
            if self.saving {
                "Applying…"
            } else {
                "Save changes"
            }
            .into(),
        );
        Self::button_text(
            cx,
            self.actions[4],
            if self.test_active {
                "Stop test"
            } else if cx.bounds().width < 760. {
                "Test mic"
            } else {
                "Test microphone"
            }
            .into(),
        );
        Self::button_text(
            cx,
            self.actions[5],
            if self.hotkey_waiting {
                "Press a key…"
            } else if cx.bounds().width < 760. {
                "Capture key"
            } else {
                "Capture hotkey"
            }
            .into(),
        );
        let _ = cx.send(
            self.actions[3],
            ButtonCommand::Disabled(!self.dirty || self.saving || !self.connected),
        );
        let _ = cx.send(
            self.preferences,
            PreferenceCommand::Sync(
                Arc::new(self.config.clone()),
                Arc::new(self.microphones.clone()),
                !self.loaded || self.saving,
            ),
        );
        self.visible_rows = (((cx.bounds().height - 520.).max(0.) / 200.) as usize + 1).clamp(1, 8);
        self.history_page = self
            .history_page
            .min(self.history.len().saturating_sub(1) / self.visible_rows);
        for (i, row) in self.rows.iter().enumerate() {
            if let Some(item) = self.history.get(self.history_page * self.visible_rows + i) {
                let _ = cx.send(*row, HistoryCommand::Sync(Arc::new(item.clone())));
            }
        }
        let _ = cx.send(
            self.actions[9],
            ButtonCommand::Disabled(self.history_page == 0),
        );
        let _ = cx.send(
            self.actions[10],
            ButtonCommand::Disabled(
                (self.history_page + 1) * self.visible_rows >= self.history.len(),
            ),
        );
        for (i, b) in self.actions.iter().enumerate() {
            let visible = match i {
                0 => self.page == 0,
                1 => self.page == 0 || self.page == 1,
                2 => {
                    self.page == 0
                        && (self.state["recording"].as_bool() == Some(true)
                            || self.state["processing"].as_bool() == Some(true))
                }
                3..=5 => self.page == 2,
                6..=7 => self.page == 3,
                8 => !self.notice.is_empty(),
                9..=10 => self.page == 1,
                11 => self.page == 3,
                12 => self.page == 1,
                _ => false,
            };
            let _ = cx.show(*b, visible);
        }
        for (i, b) in self.rows.iter().enumerate() {
            let _ = cx.show(
                *b,
                self.page == 1
                    && i < self.visible_rows
                    && self
                        .history
                        .get(self.history_page * self.visible_rows + i)
                        .is_some(),
            );
        }
        let _ = cx.show(self.search, self.page == 1);
        let _ = cx.show(self.preferences, self.page == 2);
        let _ = cx.show(self.transcript, self.page == 0 || self.page == 3);
        let _ = cx.show(self.recording, self.page == 0);
        let _ = cx.show(self.meter, self.page == 2 && self.test_active);
        let _ = cx.show(self.section, self.page == 0);
        let _ = cx.show(self.hotkey, self.page == 2);
        let key = key_name(
            self.state["hotkey"]
                .as_str()
                .or(self.config["input"]["trigger_key"].as_str())
                .unwrap_or("your hotkey"),
        );
        let _ = cx.send(
            self.recording,
            RecordingCommand::Status(self.status.clone(), key.clone()),
        );
        let _ = cx.send(self.hotkey, format!("Dictation hotkey: {key}"));
        let _ = cx.send(
            self.title,
            ["Dictation", "History", "Settings", "Service status"][self.page].into(),
        );
        let _ = cx.send(
            self.subtitle,
            [
                "Private speech recognition, on your device.",
                "Your recordings and transcriptions.",
                "Changes apply when you save.",
                "Connection details and recent service activity.",
            ][self.page]
                .into(),
        );
        let _ = cx.send(
            self.feedback,
            if self.notice.is_empty() {
                if self.connected {
                    "Speech service connected".into()
                } else {
                    "Connecting to speech service...".into()
                }
            } else {
                self.notice.clone()
            },
        );
        cx.relayout();
        cx.repaint();
    }
    fn backend(&mut self, cx: &mut Update<'_, Self>, v: &Value) {
        match v["type"].as_str().unwrap_or("") {
            "connected" => {
                self.connected = true;
                self.request(cx, json!({"cmd":"get_config"}));
            }
            "shutdown" => {
                let _ = cx.close_window();
                return;
            }
            "disconnected" => {
                self.closing = false;
                if self.saving {
                    self.notice =
                        "Connection lost while saving. Reconnect and check your settings.".into();
                }
                self.saving = false;
                self.connected = false;
                self.status = "Speech service disconnected. Reconnecting…".into();
            }
            "state" => {
                self.connected = true;
                self.state = v.clone();
                self.status = if v["recording"].as_bool() == Some(true) {
                    if v["capture_ready"].as_bool() == Some(true) {
                        "Listening to you"
                    } else {
                        "Opening microphone"
                    }
                } else if v["processing"].as_bool() == Some(true) {
                    "Turning speech into text"
                } else if v["listening"].as_bool() != Some(true) {
                    "Dictation is paused"
                } else if v["model_ready"].as_bool() != Some(true) {
                    "Loading your speech model"
                } else {
                    "Ready when you are"
                }
                .into();
                if let Some(e) = v["last_error"].as_str().filter(|s| !s.is_empty()) {
                    self.notice = e.into();
                }
                if v["recording"].as_bool() != Some(true) {
                    let _ = cx.send(self.recording, RecordingCommand::Level(Level(0., false)));
                }
            }
            "audio_level" => {
                let _ = cx.send(
                    self.recording,
                    RecordingCommand::Level(Level(v["level"].as_f64().unwrap_or(0.) as f32, true)),
                );
                return;
            }
            "transcription" => {
                self.transcript_text = v["text"].as_str().unwrap_or("").into();
                if self.page == 0 {
                    let _ = cx.send(self.transcript, Edit::Set(self.transcript_text.clone()));
                }
                if self.page == 1 {
                    self.request(cx, json!({"cmd":"history","search":self.search_text}));
                }
            }
            "transcription_preview" => {
                self.transcript_text = format!(
                    "{}{}",
                    v["committed"].as_str().unwrap_or(""),
                    v["tentative"].as_str().unwrap_or("")
                );
                if self.page == 0 {
                    let _ = cx.send(self.transcript, Edit::Set(self.transcript_text.clone()));
                }
            }
            "config" => {
                self.state["log_level"] = v["config"]["logging"]["level"].clone();
                if self.loaded {
                    self.config["logging"]["level"] = v["config"]["logging"]["level"].clone();
                }
                if !self.dirty && !self.saving {
                    self.config = v["config"].clone();
                    self.loaded = true;
                }
                self.microphones = v["microphones"].as_array().cloned().unwrap_or_default();
            }
            "config_reloaded" => {
                if self.saving {
                    self.dirty = false;
                    self.notice = "Settings applied.".into();
                }
                self.saving = false;
                if self.closing {
                    self.closing = false;
                    let _ = cx.close_window();
                } else {
                    self.request(cx, json!({"cmd":"get_config"}));
                }
            }
            "history" => {
                if v["search"].as_str().unwrap_or("") == self.search_text {
                    self.history = v["items"].as_array().cloned().unwrap_or_default();
                    if self.page == 1 {
                        self.diagnostics = self
                            .history
                            .first()
                            .and_then(|i| i["text"].as_str())
                            .unwrap_or("")
                            .into();
                        let _ = cx.send(self.transcript, Edit::Set(self.diagnostics.clone()));
                    }
                    self.history_page = self
                        .history_page
                        .min(self.history.len().saturating_sub(1) / self.visible_rows);
                }
            }
            "history_changed" => {
                self.request(cx, json!({"cmd":"history","search":self.search_text}));
            }
            "diagnostics" => {
                self.diagnostics = format!(
                    "{}\n\nRecent logs\n{}",
                    v["text"].as_str().unwrap_or(""),
                    v["logs"].as_str().unwrap_or("")
                );
                if self.page == 3 {
                    let _ = cx.after(self.log_timer, Duration::from_secs(3));
                    let _ = cx.send(self.transcript, Edit::Set(self.diagnostics.clone()));
                }
            }
            "microphone_test" => {
                self.test_active = v["done"].as_bool() != Some(true);
                let _ = cx.send(self.meter, v["level"].as_f64().unwrap_or(0.) as f32);
                self.notice = v["message"].as_str().unwrap_or("").into();
                let _ = cx.send(
                    self.recording,
                    RecordingCommand::Level(Level(
                        v["level"].as_f64().unwrap_or(0.) as f32,
                        self.test_active,
                    )),
                );
            }
            "hotkey_waiting" => self.hotkey_waiting = true,
            "hotkey_timeout" => {
                self.hotkey_waiting = false;
                self.notice = "No key received. Try again.".into();
            }
            "hotkey" => {
                self.hotkey_waiting = false;
                self.config["input"]["trigger_key"] = v["key"].clone();
                self.dirty = true;
                self.notice = "Hotkey captured. Save changes to use it.".into();
            }
            "error" => {
                self.closing = false;
                self.notice = v["message"]
                    .as_str()
                    .unwrap_or("An operation failed.")
                    .into();
                self.saving = false;
                self.hotkey_waiting = false;
            }
            _ => {}
        }
        self.refresh(cx);
    }
}

fn key_name(key: &str) -> String {
    match key {
        "KEY_RIGHTCTRL" => "Right Ctrl".into(),
        "KEY_LEFTCTRL" => "Left Ctrl".into(),
        "KEY_RIGHTALT" => "Right Alt".into(),
        "KEY_LEFTALT" => "Left Alt".into(),
        "KEY_RIGHTSHIFT" => "Right Shift".into(),
        "KEY_LEFTSHIFT" => "Left Shift".into(),
        other => other.trim_start_matches("KEY_").replace('_', " "),
    }
}
fn theme() -> Theme {
    Theme::dark()
}
fn fonts() -> Result<fire_ui_fonts::Fonts, String> {
    let path = fire_ui_text::system_font()
        .ok_or("No system font found; set FIRE_UI_FONT to a font file")?;
    fire_ui_fonts::Fonts::load(&[path])
}
impl Widget for Desktop {
    type Command = Command;
    type Output = Output;
    fn lifecycle(&mut self, cx: &mut Update<'_, Self>, event: Lifecycle) {
        if event == Lifecycle::Resized {
            self.refresh(cx);
        }
        if event == Lifecycle::Mount {
            let _ = cx.send(self.actions[3], ButtonCommand::Style(ButtonStyle::Primary));
            let _ = cx.emit(Output::Start);
            self.refresh(cx);
        }
    }
    fn update(&mut self, cx: &mut Update<'_, Self>, cmd: Command) {
        match cmd {
            Command::Backend(v) => self.backend(cx, &v),
            Command::Page(p) => self.set_page(cx, p),
            Command::Action(i) => match i {
                0 => self.request(cx, json!({"cmd":"toggle_listening"})),
                1 => {
                    let text = if self.page == 1 {
                        self.diagnostics.clone()
                    } else {
                        self.transcript_text.clone()
                    };
                    let _ = cx.copy(text);
                    self.notice = "Copied to clipboard.".into();
                    self.refresh(cx);
                }
                2 => self.request(cx, json!({"cmd":"cancel"})),
                3 => {
                    self.saving = true;
                    self.request(cx, json!({"cmd":"save_config","config":self.config}));
                    self.refresh(cx);
                }
                4 => {
                    self.test_active = !self.test_active;
                    self.request(cx,json!({"cmd":"test_microphone","node":self.config["audio"]["pipewire_node"]}));
                    self.refresh(cx);
                }
                5 => {
                    self.hotkey_waiting = true;
                    self.request(cx, json!({"cmd":"capture_hotkey"}));
                    self.refresh(cx);
                }
                6 => {
                    if self.page == 3 {
                        self.request(cx, json!({"cmd":"diagnostics"}));
                    } else {
                        self.request(cx, json!({"cmd":"get_config"}));
                    }
                }
                7 => {
                    let _ = cx.copy(self.diagnostics.clone());
                    self.notice = "Report copied.".into();
                    self.refresh(cx);
                }
                8 => {
                    self.notice.clear();
                    self.request(cx, json!({"cmd":"clear_error"}));
                    self.refresh(cx);
                }
                9 => {
                    self.history_page = self.history_page.saturating_sub(1);
                    self.refresh(cx);
                }
                10 => {
                    if (self.history_page + 1) * self.visible_rows < self.history.len() {
                        self.history_page += 1;
                    }
                    self.refresh(cx);
                }
                11 => {
                    let debug = self.state["log_level"]
                        .as_str()
                        .or(self.config["logging"]["level"].as_str())
                        == Some("DEBUG");
                    self.request(
                        cx,
                        json!({"cmd":"set_log_level","value":if debug {"INFO"} else {"DEBUG"}}),
                    );
                }
                12 => self.request(cx, json!({"cmd":"open_recordings"})),
                _ => {}
            },
            Command::Setting(i, value) => {
                if self.loaded && !self.saving {
                    let (_, section, key) = FIELDS[i];
                    self.config[section][key] = value.clone();
                    if i == 0 {
                        self.config["audio"]["device"] = json!(if value.as_str() == Some("") {
                            "default"
                        } else {
                            "pipewire"
                        });
                    }
                    self.dirty = true;
                    self.refresh(cx);
                }
            }
            Command::History(i, action) => {
                if let Some(item) = self.history.get(self.history_page * self.visible_rows + i) {
                    let id = item["id"].as_i64().unwrap_or_default();
                    match action {
                        0 => {
                            let _ = cx.copy(item["text"].as_str().unwrap_or("").to_owned());
                            self.notice = "Copied to clipboard.".into();
                        }
                        1 => self.request(cx, json!({"cmd":"favorite_history","id":id,"favorite":!item["favorite"].as_bool().unwrap_or(false)})),
                        2 => self.request(cx, json!({"cmd":"retry_history","id":id})),
                        3 => self.request(cx, json!({"cmd":"delete_history","id":id})),
                        4 => self.request(cx, json!({"cmd":"play_history","id":id})),
                        _ => {}
                    }
                    self.refresh(cx);
                }
            }
            Command::Search(EditorOutput::Changed { text, .. }) => {
                self.search_text = text.to_string();
                self.history_page = 0;
                let _ = cx.after(self.search_timer, Duration::from_millis(180));
            }
            Command::Transcript(EditorOutput::Changed { text, .. }) => {
                if self.page == 0 {
                    self.transcript_text = text.to_string();
                } else {
                    self.diagnostics = text.to_string();
                }
            }
            Command::Language(EditorOutput::Changed { text, .. }) => {
                if self.loaded {
                    self.config["transcription"]["language"] = if text.trim().is_empty() {
                        Value::Null
                    } else {
                        json!(text.trim())
                    };
                    self.dirty = true;
                    self.refresh(cx);
                }
            }
            _ => {}
        }
    }
    fn timer(&mut self, cx: &mut Update<'_, Self>, timer: Timer) {
        if timer == self.search_timer {
            self.request(cx, json!({"cmd":"history","search":self.search_text}));
        }
        if timer == self.log_timer && self.page == 3 {
            self.request(cx, json!({"cmd":"diagnostics"}));
        }
    }
    fn close_requested(&mut self, cx: &mut Update<'_, Self>) -> bool {
        if self.saving {
            self.closing = true;
            self.notice = "Finishing your settings change before closing…".into();
            self.refresh(cx);
            false
        } else {
            true
        }
    }
    fn layout(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        self.layout_view(cx, c)
    }
    fn paint(&self, cx: &mut Paint<'_>) {
        cx.painter
            .rect(cx.bounds, 0., theme().color.background.into());
    }
}
fn main() -> Result<(), String> {
    if std::env::args().any(|a| a == "--hud") {
        return hud::run();
    }
    let demo = std::env::args().any(|a| a == "--demo");
    let mut connection: Option<bridge::Bridge> = None;
    let mut state = json!({"type":"state","listening":true,"recording":false,"processing":false,"model_ready":true,"hotkey":"KEY_RIGHTCTRL","output_method":"xdotool","last_error":""});
    let fonts = fonts()?;
    run_with(
        Desktop::new(),
        WindowOptions {
            title: "Voice Dictation".into(),
            size: Size::new(1060., 860.),
            min_size: Size::new(520., 600.),
            background: theme().color.background,
            ..WindowOptions::default()
        },
        fire_ui_text::Text::new(fonts.clone())?,
        fire_ui_cairo::Cairo { fonts },
        move |output, wake| {
            if demo {
                bridge::demo(output, wake, &mut state);
                return;
            }
            let result = match output {
                Output::Start => bridge::Bridge::start(wake.clone()).map(|b| connection = Some(b)),
                Output::Request(v) => connection
                    .as_ref()
                    .ok_or_else(|| "Desktop adapter is unavailable.".to_string())
                    .and_then(|b| b.send(v)),
            };
            if let Err(message) = result {
                let _ = wake.post(Command::Backend(Arc::new(
                    json!({"type":"error","message":message}),
                )));
            }
        },
    )
}

#[cfg(test)]
mod tests;
