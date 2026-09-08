use fire_ui::*;
use fire_ui_native::{run_with, WindowOptions};
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
    levels: VecDeque<f32>,
    label: Option<Arc<Paragraph>>,
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
                        if !recording {
                            self.levels.clear();
                        }
                        cx.relayout();
                    }
                }
                if let Some(level) = state["level"].as_f64() {
                    self.levels.push_back((level as f32).clamp(0., 1.));
                    if self.levels.len() > 16 {
                        self.levels.pop_front();
                    }
                    cx.repaint();
                }
            }
        }
    }
    fn layout(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        self.label = Some(cx.paragraph(TextRequest {
            text: Arc::from(if self.recording {
                "Recording"
            } else {
                "Transcribing"
            }),
            style: TextStyle { size: 15., font: 0 },
            width: Some(135.),
            revision: 0,
        }));
        Metrics::new(c.constrain(Size::new(280., 58.)))
    }
    fn paint(&self, cx: &mut Paint<'_>) {
        let accent = if self.recording {
            super::GREEN
        } else {
            super::BLUE
        };
        cx.painter.rect(
            Rect::new(0., 0., cx.bounds.width, cx.bounds.height),
            0.,
            super::PANEL.into(),
        );
        cx.painter
            .rect(Rect::new(0., 0., 3., cx.bounds.height), 0., accent.into());
        cx.painter
            .rect(Rect::new(18., 25., 8., 8.), 4., accent.into());
        if let Some(label) = &self.label {
            cx.painter
                .paragraph(label, Point::new(38., 18.), super::TEXT.into());
        }
        for i in 0..16 {
            let h = 3. + self.levels.get(i).copied().unwrap_or(0.) * 29.;
            cx.painter.rect(
                Rect::new(178. + i as f32 * 5., (58. - h) / 2., 2.5, h),
                1.25,
                accent.into(),
            );
        }
    }
}

pub fn run() -> Result<(), String> {
    let position = std::env::var("VOICE_DICTATION_HUD_POSITION")
        .ok()
        .and_then(|s| {
            let (x, y) = s.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        });
    run_with(
        Element::leaf(Hud {
            recording: true,
            levels: VecDeque::new(),
            label: None,
        }),
        WindowOptions {
            font: Some(
                fire_ui_native::system_font()
                    .ok_or("No system font found; set FIRE_UI_FONT to a font file")?,
            ),
            title: "Voice Dictation Status".into(),
            overlay: true,
            size: Size::new(280., 58.),
            min_size: Size::new(280., 58.),
            position,
            background: super::PANEL,
            ..WindowOptions::default()
        },
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
