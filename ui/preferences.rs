use super::*;

/// A caption and a naturally measured control. The framework owns their sizing.
pub(super) struct Field<W: Widget> {
    label: Child<Label>,
    control: Child<W>,
}
impl<W: Widget> Field<W> {
    fn new(label: &str, control: Element<W>) -> Element<Self> {
        Element::build(|c| Self {
            label: c.add(Element::leaf(Label::toned(
                label,
                TextRole::Small,
                ColorRole::Muted,
            ))),
            control: c.bubble(control),
        })
    }
}
impl<W: Widget> Widget for Field<W> {
    type Command = W::Command;
    type Output = W::Output;
    fn update(&mut self, cx: &mut Update<'_, Self>, command: Self::Command) {
        let _ = cx.send(self.control, command);
    }
    fn layout(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        column(
            cx,
            c,
            Flow::gap(gap(cx, 0.75)).align(Align::Stretch),
            &[Entry::natural(self.label), Entry::natural(self.control)],
        )
    }
}

pub(super) enum PreferenceCommand {
    Sync(
        Arc<Value>,
        Arc<Vec<Value>>,
        bool,
        Arc<str>,
        Arc<str>,
        Arc<str>,
        Arc<str>,
        bool,
        bool,
    ),
    Level(f32),
    Action(usize),
    Select(usize, usize),
    Toggle(usize, bool),
    Language(EditorOutput),
}
impl Data for PreferenceCommand {
    fn bytes(&self) -> usize {
        match self {
            Self::Sync(config, microphones, _, runtime, model, profile, hotkey, _, _) => {
                config.to_string().len()
                    + microphones
                        .iter()
                        .map(|v| v.to_string().len())
                        .sum::<usize>()
                    + runtime.len()
                    + model.len()
                    + profile.len()
                    + hotkey.len()
            }
            Self::Language(value) => value.bytes(),
            _ => 32,
        }
    }
}
pub(super) struct Preferences {
    choices: Vec<(usize, Child<Field<Dropdown>>)>,
    switches: Vec<(usize, Child<Switch>)>,
    language: Child<Field<Editor>>,
    backend: Child<Label>,
    acceleration_warning: Child<Label>,
    hotkey: Child<Label>,
    capture: Child<Control>,
    microphone: Child<Control>,
    meter: Child<Progress>,
    headings: Vec<Child<Label>>,
    values: Vec<Vec<Value>>,
    captions: Vec<Vec<Arc<str>>>,
    language_text: String,
    test_active: bool,
    test_level: f32,
    acceleration_warning_visible: bool,
}
impl Preferences {
    pub(super) fn new() -> Element<Self> {
        Element::build(|c| Self {
            choices: [0, 1, 2, 3, 6, 11, 12]
                .into_iter()
                .map(|i| {
                    (
                        i,
                        c.connect(
                            Field::new(FIELDS[i].0, Dropdown::new(FIELDS[i].0, ["Loading..."])),
                            move |v| PreferenceCommand::Select(i, *v),
                        ),
                    )
                })
                .collect(),
            switches: [4, 5, 7, 8, 9, 10]
                .into_iter()
                .map(|i| {
                    (
                        i,
                        c.connect(Switch::new(FIELDS[i].0), move |v| {
                            PreferenceCommand::Toggle(i, *v)
                        }),
                    )
                })
                .collect(),
            language: c.connect(
                Field::new(
                    "Language",
                    Element::leaf(
                        Editor::field("")
                            .placeholder("Auto detect")
                            .caret_blink(false),
                    ),
                ),
                |v| PreferenceCommand::Language(v.clone()),
            ),
            backend: c.add(Element::leaf(
                Label::toned("Backend: loading", TextRole::Small, ColorRole::Muted).wrap(),
            )),
            acceleration_warning: c.add(Element::leaf(
                Label::toned("", TextRole::Small, ColorRole::Danger).wrap(),
            )),
            hotkey: c.add(Element::leaf(
                Label::toned(
                    "Dictation hotkey: loading",
                    TextRole::Small,
                    ColorRole::Muted,
                )
                .wrap(),
            )),
            capture: c.connect(control("Capture hotkey"), |_| PreferenceCommand::Action(5)),
            microphone: c.connect(control("Test microphone"), |_| PreferenceCommand::Action(4)),
            meter: c.add(Progress::new("Microphone level", 0.)),
            headings: ["Capture & recognition", "Text output", "Feedback"]
                .into_iter()
                .map(|s| c.add(Element::leaf(Label::styled(s, TextRole::Heading))))
                .collect(),
            values: vec![vec![]; FIELDS.len()],
            captions: vec![vec![]; FIELDS.len()],
            language_text: String::new(),
            test_active: false,
            test_level: 0.,
            acceleration_warning_visible: false,
        })
    }
    fn options(index: usize, microphones: &[Value], model: &str) -> Vec<(String, Value)> {
        if index == 1 {
            return [
                ("parakeet-unified-en-0.6b", "parakeet-unified-en-0.6b"),
                ("Canary 180M Flash", "canary-180m-flash"),
                ("tiny.en", "tiny.en"),
                ("base.en", "base.en"),
                ("small.en", "small.en"),
                ("medium.en", "medium.en"),
                ("large-v3", "large-v3"),
                ("tiny", "tiny"),
                ("base", "base"),
                ("small", "small"),
                ("medium", "medium"),
            ]
            .into_iter()
            .map(|(label, value)| (label.into(), json!(value)))
            .collect();
        }
        let names: Vec<String> = match index {
            2 => ["int8", "int8_float16", "float16", "float32"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            3 => (1..=10).map(|n| n.to_string()).collect(),
            6 => [
                "auto", "xdotool", "ydotool", "dotool", "wtype", "xclip", "none",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            12 => [0, 1, 2, 4, 8, 16, 32, 64]
                .into_iter()
                .map(|n| n.to_string())
                .collect(),
            _ => vec![],
        };
        if index == 11 {
            return profile_options(model);
        }
        if index == 12 {
            return vec![
                ("Automatic · engine default".into(), json!(0)),
                ("1".into(), json!(1)),
                ("2".into(), json!(2)),
                ("4".into(), json!(4)),
                ("8".into(), json!(8)),
                ("16".into(), json!(16)),
                ("32".into(), json!(32)),
                ("64".into(), json!(64)),
            ];
        }
        if index == 0 {
            let mut result = vec![("System default".into(), json!(""))];
            result.extend(microphones.iter().map(|m| {
                (
                    m["description"]
                        .as_str()
                        .unwrap_or("Microphone")
                        .to_string(),
                    m["name"].clone(),
                )
            }));
            result
        } else {
            names
                .into_iter()
                .map(|s| {
                    let v = if index == 3 {
                        json!(s.parse::<u8>().unwrap())
                    } else {
                        json!(s)
                    };
                    (s, v)
                })
                .collect()
        }
    }
}
impl Widget for Preferences {
    type Command = PreferenceCommand;
    type Output = Command;
    fn update(&mut self, cx: &mut Update<'_, Self>, command: Self::Command) {
        match command {
            PreferenceCommand::Sync(
                config,
                microphones,
                disabled,
                runtime,
                model,
                profile,
                hotkey,
                test_active,
                hotkey_waiting,
            ) => {
                let _ = cx.send(self.backend, format_backend(&model, &runtime));
                let warning = acceleration_warning(&model, &profile, &runtime);
                self.acceleration_warning_visible = warning.is_some();
                let _ = cx.send(self.acceleration_warning, warning.unwrap_or_default());
                let _ = cx.show(self.acceleration_warning, self.acceleration_warning_visible);
                self.test_active = test_active;
                let _ = cx.send(
                    self.hotkey,
                    format!("Dictation hotkey: {}", key_name(&hotkey)),
                );
                let capture_label = if hotkey_waiting {
                    "Press a key…"
                } else if cx.bounds().width < 760. {
                    "Capture key"
                } else {
                    "Capture hotkey"
                }
                .to_owned();
                let _ = cx.send(self.capture, ButtonCommand::Content(capture_label.clone()));
                let _ = cx.send(self.capture, ButtonCommand::Label(capture_label));
                let microphone_label = if test_active {
                    "Stop test"
                } else if cx.bounds().width < 760. {
                    "Test mic"
                } else {
                    "Test microphone"
                }
                .to_owned();
                let _ = cx.send(
                    self.microphone,
                    ButtonCommand::Content(microphone_label.clone()),
                );
                let _ = cx.send(self.microphone, ButtonCommand::Label(microphone_label));
                let _ = cx.send(self.capture, ButtonCommand::Disabled(disabled));
                let _ = cx.send(self.microphone, ButtonCommand::Disabled(disabled));
                let _ = cx.send(self.meter, self.test_level);
                let _ = cx.show(self.meter, test_active);
                for &(i, child) in &self.choices {
                    let (_, section, key) = FIELDS[i];
                    let current = &config[section][key];
                    let model = config["transcription"]["model"].as_str().unwrap_or("");
                    let mut options = Self::options(i, &microphones, model);
                    if !current.is_null() && !options.iter().any(|(_, value)| value == current) {
                        options.push((
                            current
                                .as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| current.to_string()),
                            current.clone(),
                        ));
                    }
                    let captions: Vec<Arc<str>> =
                        options.iter().map(|(s, _)| Arc::from(s.as_str())).collect();
                    self.values[i] = options.into_iter().map(|(_, v)| v).collect();
                    if self.captions[i] != captions {
                        self.captions[i] = captions.clone();
                        let _ = cx.send(child, Routed::Command(DropdownCommand::Options(captions)));
                    }
                    let selected = self.values[i]
                        .iter()
                        .position(|v| v == current)
                        .unwrap_or(0);
                    let _ = cx.send(child, Routed::Command(DropdownCommand::Select(selected)));
                    let _ = cx.send(child, Routed::Command(DropdownCommand::Disabled(disabled)));
                }
                for &(i, child) in &self.switches {
                    let (_, section, key) = FIELDS[i];
                    let _ = cx.send(
                        child,
                        ToggleCommand::Checked(config[section][key].as_bool().unwrap_or(false)),
                    );
                    let _ = cx.send(child, ToggleCommand::Disabled(disabled));
                }
                let text = config["transcription"]["language"].as_str().unwrap_or("");
                if text != self.language_text {
                    self.language_text = text.into();
                    let _ = cx.send(self.language, Edit::Set(text.into()));
                }
            }
            PreferenceCommand::Level(level) => {
                self.test_level = level;
                let _ = cx.send(self.meter, level);
            }
            PreferenceCommand::Action(index) => {
                let _ = cx.emit(Command::Action(index));
            }
            PreferenceCommand::Select(i, selected) => {
                if let Some(value) = self.values[i].get(selected) {
                    let _ = cx.emit(Command::Setting(i, value.clone()));
                }
            }
            PreferenceCommand::Toggle(i, value) => {
                let _ = cx.emit(Command::Setting(i, json!(value)));
            }
            PreferenceCommand::Language(value) => {
                if let EditorOutput::Changed { text, .. } = &value {
                    self.language_text = text.to_string();
                }
                let _ = cx.emit(Command::Language(value));
            }
        }
    }
    fn layout(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        let unit = gap(cx, 1.);
        let limits = Constraints::loose(Size::new(c.max.width, f32::INFINITY));
        let mut y = 0.;
        for group in 0..3 {
            let heading = column_at(
                cx,
                Point::new(0., y),
                limits,
                Flow::default(),
                &[Entry::natural(self.headings[group])],
            );
            y += heading.size.height + unit * 2.;
            if group == 0 {
                let backend = column_at(
                    cx,
                    Point::new(0., y),
                    limits,
                    Flow::default(),
                    &[Entry::natural(self.backend)],
                );
                y += backend.size.height + unit * 2.;
                if self.acceleration_warning_visible {
                    let warning = column_at(
                        cx,
                        Point::new(0., y),
                        limits,
                        Flow::default(),
                        &[Entry::natural(self.acceleration_warning)],
                    );
                    y += warning.size.height + unit * 2.;
                }
            }
            let ids: &[usize] = match group {
                0 => &[0, 1, 11, 12, 2, 3],
                1 => &[6],
                _ => &[],
            };
            let children: Vec<LayoutChild> = ids
                .iter()
                .map(|id| self.choices.iter().find(|(i, _)| i == id).unwrap().1.into())
                .chain((group == 0).then_some(self.language.into()))
                .collect();
            if !children.is_empty() {
                let fields = grid_at(
                    cx,
                    Point::new(0., y),
                    limits,
                    0,
                    unit * 30.,
                    unit * 2.,
                    &children,
                );
                y += fields.size.height + unit * 2.;
            }
            let ids: &[usize] = match group {
                0 => &[4, 5],
                1 => &[7],
                _ => &[8, 9, 10],
            };
            let entries: Vec<Entry> = ids
                .iter()
                .map(|id| Entry::natural(self.switches.iter().find(|(i, _)| i == id).unwrap().1))
                .collect();
            let toggles = column_at(
                cx,
                Point::new(0., y),
                limits,
                Flow::gap(unit * 1.5),
                &entries,
            );
            y += toggles.size.height;
            if group == 0 {
                y += unit * 2.;
                let key = column_at(
                    cx,
                    Point::new(0., y),
                    limits,
                    Flow::default(),
                    &[Entry::natural(self.hotkey)],
                );
                y += key.size.height + unit;
                let mut actions = vec![
                    Entry::natural(self.capture),
                    Entry::natural(self.microphone),
                ];
                if self.test_active {
                    actions.push(Entry::fill(self.meter));
                }
                let tools = row_at(
                    cx,
                    Point::new(0., y),
                    limits,
                    Flow::gap(unit).align(Align::Center),
                    &actions,
                );
                y += tools.size.height;
            }
            y += unit * 4.;
        }
        Metrics::new(c.constrain(Size::new(c.max.width, y)))
    }
}

pub(super) fn format_backend(model: &str, runtime: &str) -> String {
    let engine = if model.ends_with(".en")
        || matches!(model, "tiny" | "base" | "small" | "medium" | "large-v3")
    {
        "CTranslate2"
    } else {
        "transcribe.cpp"
    };
    let mode = match runtime {
        "standard" => "CPU",
        "fast" => "CPU · Fast preprocessing",
        "adaptive" => "CPU · Adaptive short context",
        "vulkan" => "Vulkan",
        "vulkan-full" => "Vulkan · Full-sequence experimental",
        "hybrid" => "Vulkan encoder + CPU decoder",
        value if value.starts_with("CPU fallback:") => "CPU fallback",
        _ => "loading",
    };
    format!("Active backend: {engine} · {mode}")
}

pub(super) fn acceleration_warning(model: &str, requested: &str, runtime: &str) -> Option<String> {
    let gguf = matches!(model, "parakeet-unified-en-0.6b" | "canary-180m-flash")
        || model.ends_with(".gguf");
    if !gguf {
        return None;
    }
    if let Some(reason) = runtime.strip_prefix("CPU fallback:") {
        let requested = profile_name(requested);
        return Some(format!(
            "Acceleration unavailable: {requested} was requested, but inference fell back to CPU. {}",
            reason.trim()
        ));
    }
    if !cfg!(feature = "vulkan") {
        return Some(
            "Acceleration unavailable: this installation was built without Vulkan, so this model can only run on CPU. Reinstall the Vulkan build to restore GPU profiles."
                .into(),
        );
    }
    None
}

fn profile_name(profile: &str) -> &'static str {
    match profile {
        "vulkan" => "Vulkan GPU",
        "vulkan-full" => "Vulkan GPU full recording",
        "hybrid" => "Vulkan encoder + CPU decoder",
        _ => "GPU acceleration",
    }
}

pub(super) fn profile_options(model: &str) -> Vec<(String, Value)> {
    let gguf = matches!(model, "parakeet-unified-en-0.6b" | "canary-180m-flash")
        || model.ends_with(".gguf");
    let mut options = vec![("Standard CPU".into(), json!("standard"))];
    if !gguf {
        options.push(("CPU · Fast preprocessing".into(), json!("fast")));
        if model == "base.en" {
            options.push((
                "CPU · Adaptive context (experimental)".into(),
                json!("adaptive"),
            ));
        }
    }
    if cfg!(feature = "vulkan") && gguf {
        options.push(("Vulkan GPU".into(), json!("vulkan")));
        if model == "canary-180m-flash" {
            options.push(("Vulkan encoder + CPU decoder".into(), json!("hybrid")));
        }
        options.push((
            "Vulkan GPU · Full recording (experimental)".into(),
            json!("vulkan-full"),
        ));
    }
    options
}
