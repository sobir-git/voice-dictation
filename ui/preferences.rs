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
    Sync(Arc<Value>, Arc<Vec<Value>>, bool),
    Select(usize, usize),
    Toggle(usize, bool),
    Language(EditorOutput),
}
impl Data for PreferenceCommand {
    fn bytes(&self) -> usize {
        match self {
            Self::Sync(config, microphones, _) => {
                config.to_string().len()
                    + microphones
                        .iter()
                        .map(|v| v.to_string().len())
                        .sum::<usize>()
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
    headings: Vec<Child<Label>>,
    values: Vec<Vec<Value>>,
    captions: Vec<Vec<Arc<str>>>,
    language_text: String,
}
impl Preferences {
    pub(super) fn new() -> Element<Self> {
        Element::build(|c| Self {
            choices: [0, 1, 2, 3, 6]
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
            headings: ["Capture & recognition", "Text output", "Feedback"]
                .into_iter()
                .map(|s| c.add(Element::leaf(Label::styled(s, TextRole::Heading))))
                .collect(),
            values: vec![vec![]; FIELDS.len()],
            captions: vec![vec![]; FIELDS.len()],
            language_text: String::new(),
        })
    }
    fn options(index: usize, microphones: &[Value]) -> Vec<(String, Value)> {
        let names: Vec<String> = match index {
            1 => [
                "tiny.en",
                "base.en",
                "small.en",
                "medium.en",
                "large-v3",
                "tiny",
                "base",
                "small",
                "medium",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
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
            _ => vec![],
        };
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
            PreferenceCommand::Sync(config, microphones, disabled) => {
                for &(i, child) in &self.choices {
                    let (_, section, key) = FIELDS[i];
                    let current = &config[section][key];
                    let mut options = Self::options(i, &microphones);
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
            let ids: &[usize] = match group {
                0 => &[0, 1, 2, 3],
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
            y += toggles.size.height + unit * 4.;
        }
        Metrics::new(c.constrain(Size::new(c.max.width, y)))
    }
}
