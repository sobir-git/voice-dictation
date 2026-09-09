use fire_ui::*;
use fire_ui_native::{run_with, WindowOptions};
use fire_ui_widgets::*;
use serde_json::Value;
use std::{io::BufRead, sync::Arc};

#[derive(Clone)]
enum Command {
    State(Arc<Value>),
    Close,
}
impl Data for Command {
    fn bytes(&self) -> usize {
        256
    }
}
struct Hud {
    recording: bool,
    label: Child<Label>,
    wave: Child<super::Wave>,
}
impl Widget for Hud {
    type Command = Command;
    type Output = ();
    fn lifecycle(&mut self, cx: &mut Update<'_, Self>, event: Lifecycle) {
        if event == Lifecycle::Mount {
            let _ = cx.emit(());
        }
    }
    fn update(&mut self, cx: &mut Update<'_, Self>, command: Command) {
        match command {
            Command::Close => {
                let _ = cx.close_window();
            }
            Command::State(state) => {
                if let Some(recording) = state["recording"].as_bool() {
                    if self.recording != recording {
                        self.recording = recording;
                        let _ = cx.send(
                            self.label,
                            if recording {
                                "Recording"
                            } else {
                                "Transcribing"
                            }
                            .into(),
                        );
                        let _ = cx.send(self.wave, super::Level(0., recording));
                    }
                }
                if let Some(level) = state["level"].as_f64() {
                    let _ = cx.send(self.wave, super::Level(level as f32, self.recording));
                }
            }
        }
    }
    fn layout(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        let inset = gap(cx, 1.5);
        row_at(
            cx,
            Point::new(inset, inset),
            c.inset(inset),
            Flow::gap(gap(cx, 2.)).align(Align::Center),
            &[Entry::natural(self.label), Entry::fill(self.wave)],
        );
        Metrics::new(c.max)
    }
    fn paint(&self, cx: &mut Paint<'_>) {
        cx.painter
            .rect(cx.bounds, 0., super::theme().color.surface.into());
    }
}

pub fn run() -> Result<(), String> {
    let position = std::env::var("VOICE_DICTATION_HUD_POSITION")
        .ok()
        .and_then(|s| {
            let (x, y) = s.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        });
    let fonts = super::fonts()?;
    run_with(
        Element::build(|c| Hud {
            recording: true,
            label: c.add(Element::leaf(Label::styled("Recording", TextRole::Body))),
            wave: c.add(Element::leaf(super::Wave {
                levels: std::collections::VecDeque::new(),
                active: true,
            })),
        }),
        WindowOptions {
            title: "Voice Dictation Status".into(),
            overlay: true,
            size: Size::new(280., 58.),
            min_size: Size::new(280., 58.),
            position,
            background: super::theme().color.surface,
            ..WindowOptions::default()
        },
        fire_ui_text::Text::new(fonts.clone())?,
        fire_ui_cairo::Cairo { fonts },
        |(), wake| {
            let wake = wake.clone();
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    if let Ok(state) = serde_json::from_str(&line) {
                        let _ = wake.post(Command::State(Arc::new(state)));
                    }
                }
                let _ = wake.post(Command::Close);
            });
        },
    )
}
