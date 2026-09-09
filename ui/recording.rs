use super::*;

pub(super) enum RecordingCommand {
    Status(String, String),
    Level(Level),
}
impl Data for RecordingCommand {
    fn bytes(&self) -> usize {
        match self {
            Self::Status(a, b) => a.len() + b.len(),
            Self::Level(_) => 8,
        }
    }
}
pub(super) struct Recording {
    status: Child<Label>,
    hint: Child<Label>,
    wave: Child<Wave>,
}
impl Recording {
    pub(super) fn new() -> Element<Self> {
        Element::build(|c| Self {
            status: c.add(Element::leaf(
                Label::styled("Connecting...", TextRole::Title).wrap(),
            )),
            hint: c.add(Element::leaf(
                Label::toned("", TextRole::Body, ColorRole::Muted).wrap(),
            )),
            wave: c.add(Element::leaf(Wave {
                levels: VecDeque::new(),
                active: false,
            })),
        })
    }
}
impl Widget for Recording {
    type Command = RecordingCommand;
    type Output = std::convert::Infallible;
    fn update(&mut self, cx: &mut Update<'_, Self>, command: Self::Command) {
        match command {
            RecordingCommand::Status(status, key) => {
                let _ = cx.send(self.status, status);
                let _ = cx.send(self.hint, format!("Hold {key} to speak. Release to type."));
            }
            RecordingCommand::Level(level) => {
                let _ = cx.send(self.wave, level);
            }
        }
    }
    fn layout(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        column(
            cx,
            c,
            Flow::gap(gap(cx, 1.5)).align(Align::Stretch),
            &[
                Entry::natural(self.status),
                Entry::natural(self.hint),
                Entry::natural(self.wave),
            ],
        )
    }
}
