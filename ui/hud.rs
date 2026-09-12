use fire_ui::*;
use fire_ui_native::{run_with, WindowOptions};
use fire_ui_widgets::*;
use serde_json::Value;
use std::{collections::VecDeque, io::BufRead, sync::Arc};

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
    wave: Child<HudWave>,
}
struct HudWave {
    levels: VecDeque<f32>,
    recording: bool,
    phase: f32,
}
impl Widget for HudWave {
    type Command = super::Level;
    type Output = std::convert::Infallible;
    fn update(&mut self, cx: &mut Update<'_, Self>, super::Level(level, recording): Self::Command) {
        self.recording = recording;
        if recording {
            self.levels.push_back(level.clamp(0., 1.));
            if self.levels.len() > 18 {
                self.levels.pop_front();
            }
            cx.cancel_frame();
        } else {
            self.levels.clear();
            cx.request_frame();
        }
        cx.repaint();
    }
    fn lifecycle(&mut self, cx: &mut Update<'_, Self>, event: Lifecycle) {
        if event == Lifecycle::Visibility(true) && !self.recording {
            cx.request_frame();
        }
    }
    fn frame(&mut self, cx: &mut Update<'_, Self>, time: FrameTime) {
        if self.recording {
            return;
        }
        self.phase = (self.phase + time.elapsed.as_secs_f32().min(0.04) * 4.)
            .rem_euclid(std::f32::consts::TAU);
        cx.repaint();
        cx.request_frame();
    }
    fn layout(&mut self, _: &mut Layout<'_>, c: Constraints) -> Metrics {
        Metrics::new(c.constrain(Size::new(c.max.width, 28.)))
    }
    fn paint(&self, cx: &mut Paint<'_>) {
        let count = if self.recording { 18 } else { 7 };
        let gap = if self.recording { 2.5 } else { 5. };
        let width = (cx.bounds.width - gap * (count - 1) as f32) / count as f32;
        for i in 0..count {
            let height = if self.recording {
                5. + self.levels.get(i).copied().unwrap_or(0.) * 20.
            } else {
                let travel = (self.phase - i as f32 * 0.65).sin();
                8. + (travel + 1.) * 8.
            };
            let color = if self.recording {
                super::theme().color.success.alpha(0.9)
            } else {
                super::theme().color.accent.alpha(0.9)
            };
            cx.painter.rect(
                Rect::new(
                    i as f32 * (width + gap),
                    (cx.bounds.height - height) / 2.,
                    width,
                    height,
                ),
                width / 2.,
                color.into(),
            );
        }
    }
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
    fn input(&mut self, cx: &mut Update<'_, Self>, _: Phase, input: &Input) {
        if matches!(
            input,
            Input::Button {
                button: 1,
                down: true,
                ..
            }
        ) {
            let _ = cx.window(WindowAction::Drag);
            cx.stop();
        }
    }
    fn cursor(&self, _: Point) -> Option<CursorIcon> {
        Some(CursorIcon::Arrow)
    }
    fn layout(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        let inset = gap(cx, 0.75);
        column_at(
            cx,
            Point::new(inset, inset),
            c.inset(inset),
            Flow::gap(gap(cx, 0.25)).align(Align::Center),
            &[Entry::fill(self.wave), Entry::natural(self.label)],
        );
        Metrics::new(c.max)
    }
    fn paint(&self, cx: &mut Paint<'_>) {
        cx.painter.rect(
            cx.bounds,
            7.,
            super::theme().color.surface.alpha(0.72).into(),
        );
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
            label: c.add(Element::leaf(Label::styled("Recording", TextRole::Small))),
            wave: c.add(Element::leaf(HudWave {
                levels: VecDeque::new(),
                recording: true,
                phase: 0.,
            })),
        }),
        WindowOptions {
            title: "Voice Dictation Status".into(),
            overlay: true,
            decorations: false,
            transparent: true,
            click_through: Some(false),
            size: Size::new(168., 60.),
            min_size: Size::new(168., 60.),
            position,
            background: super::theme().color.surface.alpha(0.),
            ..WindowOptions::default()
        },
        fire_ui_text::Text::new(fonts.clone())?,
        fire_ui_cairo::Cairo::direct(fonts),
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
