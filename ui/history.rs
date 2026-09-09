use super::*;

#[derive(Clone)]
pub(super) enum HistoryCommand {
    Sync(Arc<Value>),
    Action(usize),
}
impl Data for HistoryCommand {
    fn bytes(&self) -> usize {
        match self {
            Self::Sync(value) => value.to_string().len(),
            Self::Action(_) => 8,
        }
    }
}

pub(super) struct HistoryRow {
    timestamp: Child<Label>,
    text: Child<Label>,
    play: Child<Control>,
    progress: Child<Progress>,
    elapsed: Child<Label>,
    duration: Child<Label>,
    actions: Vec<Child<Control>>,
    failed: bool,
}

impl HistoryRow {
    pub(super) fn new() -> Element<Self> {
        Element::build(|c| Self {
            timestamp: c.add(Element::leaf(Label::styled("", TextRole::Heading))),
            text: c.add(Element::leaf(
                Label::toned("", TextRole::Body, ColorRole::Muted).wrap(),
            )),
            play: c.connect(control("Play"), |_| HistoryCommand::Action(4)),
            progress: c.discard(Progress::new("Recording position", 0.)),
            elapsed: c.add(Element::leaf(Label::toned(
                "0:00",
                TextRole::Small,
                ColorRole::Muted,
            ))),
            duration: c.add(Element::leaf(Label::toned(
                "0:00",
                TextRole::Small,
                ColorRole::Muted,
            ))),
            actions: ["Copy", "Favorite", "Re-transcribe", "Delete"]
                .into_iter()
                .enumerate()
                .map(|(i, label)| {
                    c.connect(
                        Button::styled(Element::leaf(Label::new(label)), label, ButtonStyle::Ghost),
                        move |_| HistoryCommand::Action(i),
                    )
                })
                .collect(),
            failed: false,
        })
    }
}

impl Widget for HistoryRow {
    type Command = HistoryCommand;
    type Output = usize;

    fn update(&mut self, cx: &mut Update<'_, Self>, command: Self::Command) {
        match command {
            HistoryCommand::Sync(item) => {
                let failed = item["failed"].as_bool().unwrap_or(false);
                self.failed = failed;
                let text = if failed {
                    "Transcription failed. You can re-transcribe using the retry action."
                } else {
                    item["text"].as_str().unwrap_or("")
                };
                let _ = cx.send(
                    self.timestamp,
                    format_timestamp(item["timestamp"].as_str().unwrap_or("")),
                );
                let _ = cx.send(self.text, text.to_owned());
                let duration = format_duration(item["duration"].as_f64().unwrap_or(0.));
                let _ = cx.send(self.duration, duration);
                let has_audio = item["audio_path"]
                    .as_str()
                    .is_some_and(|path| !path.is_empty());
                let _ = cx.send(self.play, ButtonCommand::Disabled(!has_audio));
                let favorite = item["favorite"].as_bool().unwrap_or(false);
                let _ = cx.send(
                    self.actions[1],
                    ButtonCommand::Content(if favorite { "Favorited" } else { "Favorite" }.into()),
                );
                let _ = cx.send(
                    self.actions[1],
                    ButtonCommand::Label(
                        if favorite {
                            "Remove favorite"
                        } else {
                            "Favorite"
                        }
                        .into(),
                    ),
                );
                let _ = cx.send(self.actions[2], ButtonCommand::Disabled(!has_audio));
                cx.relayout();
            }
            HistoryCommand::Action(action) => {
                let _ = cx.emit(action);
            }
        }
    }

    fn layout(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        let unit = gap(cx, 1.);
        let natural = Constraints::loose(Size::new(c.max.width, f32::INFINITY));
        let heading = row(
            cx,
            natural,
            Flow::gap(unit).align(Align::Center),
            &[
                Entry::fill(self.timestamp),
                Entry::natural(self.actions[0]),
                Entry::natural(self.actions[1]),
                Entry::natural(self.actions[2]),
                Entry::natural(self.actions[3]),
            ],
        );
        let text_y = heading.size.height + unit;
        let body = column_at(
            cx,
            Point::new(0., text_y),
            natural,
            Flow::default(),
            &[Entry::natural(self.text)],
        );
        let audio_y = text_y + body.size.height + unit * 1.5;
        let audio = row_at(
            cx,
            Point::new(0., audio_y),
            natural,
            Flow::gap(unit).align(Align::Center),
            &[
                Entry::natural(self.play),
                Entry::natural(self.elapsed),
                Entry::fill(self.progress),
                Entry::natural(self.duration),
            ],
        );
        Metrics::new(c.constrain(Size::new(c.max.width, audio_y + audio.size.height)))
    }
}

fn format_duration(seconds: f64) -> String {
    let seconds = seconds.max(0.).round() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn format_timestamp(timestamp: &str) -> String {
    let Some((date, time)) = timestamp.split_once(' ') else {
        return timestamp.to_owned();
    };
    let mut date = date.split('-').filter_map(|part| part.parse::<i32>().ok());
    let (Some(year), Some(month), Some(day)) = (date.next(), date.next(), date.next()) else {
        return timestamp.to_owned();
    };
    let mut time = time.split(':').filter_map(|part| part.parse::<u32>().ok());
    let (Some(hour), Some(minute)) = (time.next(), time.next()) else {
        return timestamp.to_owned();
    };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(month) = months.get(month.saturating_sub(1) as usize) else {
        return timestamp.to_owned();
    };
    let suffix = if hour < 12 { "AM" } else { "PM" };
    let hour = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    if year == current_year() {
        format!("{month} {day}, {hour}:{minute:02} {suffix}")
    } else {
        format!("{month} {day}, {year}, {hour}:{minute:02} {suffix}")
    }
}

fn current_year() -> i32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    year as i32
}
