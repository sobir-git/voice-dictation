use super::*;

impl Desktop {
    pub(super) fn layout_view(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        self.labels.clear();
        let w = c.max.width;
        let h = c.max.height;
        self.sidebar = if w < 760. { 142. } else { 184. };
        let x = self.sidebar + 32.;
        let width = (w - x - 32.).max(220.);
        self.content = Rect::new(x, 0., width, h);
        self.caption(
            cx,
            "Voice\nDictation",
            Point::new(24., 30.),
            if w < 760. { 20. } else { 24. },
            TEXT,
            self.sidebar - 35.,
        );
        self.caption(
            cx,
            "Local speech,\nyour own words.",
            Point::new(24., 103.),
            12.,
            MUTED,
            self.sidebar - 35.,
        );
        for (i, b) in self.nav.iter().enumerate() {
            place(
                cx,
                *b,
                Rect::new(14., 174. + i as f32 * 48., self.sidebar - 28., 40.),
            );
        }
        if h > 520. {
            self.caption(
                cx,
                if self.connected {
                    "Speech service connected"
                } else {
                    "Connecting to service"
                },
                Point::new(24., h - 70.),
                11.,
                if self.connected { GREEN } else { MUTED },
                self.sidebar - 35.,
            );
        }
        self.caption(cx, PAGES[self.page], Point::new(x, 30.), 28., TEXT, width);
        let subtitle = match self.page {
            0 => "Hold your hotkey. Speak. Release.",
            1 => "Your recent dictations stay on this device.",
            2 => {
                if h < 780. {
                    "Scroll to see all settings."
                } else {
                    "Choose how you speak, and how your words arrive."
                }
            }
            _ => "Service health and recent activity.",
        };
        self.caption(cx, subtitle, Point::new(x, 73.), 13., MUTED, width);
        // Hidden controls keep their state but do not receive input or layout work.
        let show = |cx: &mut Layout<'_>, b: Child<Control>, r: Rect| {
            place(cx, b, r);
        };
        match self.page {
            0 => {
                let key = self.state["hotkey"]
                    .as_str()
                    .or(self.config["input"]["trigger_key"].as_str())
                    .unwrap_or("your hotkey");
                let key = key_name(key);
                self.caption(
                    cx,
                    self.status.clone(),
                    Point::new(x + 24., 129.),
                    24.,
                    TEXT,
                    width - 48.,
                );
                self.caption(
                    cx,
                    format!("Hold {key}"),
                    Point::new(x + 24., 174.),
                    14.,
                    BLUE,
                    width - 48.,
                );
                place(cx, self.wave, Rect::new(x + 26., 216., width - 52., 68.));
                show(cx, self.actions[0], Rect::new(x + 24., 310., 168., 38.));
                if self.state["recording"].as_bool() == Some(true)
                    || self.state["processing"].as_bool() == Some(true)
                {
                    show(cx, self.actions[2], Rect::new(x + 206., 310., 160., 38.));
                }
                self.caption(
                    cx,
                    "Latest dictation",
                    Point::new(x, 395.),
                    16.,
                    TEXT,
                    width - 135.,
                );
                show(
                    cx,
                    self.actions[1],
                    Rect::new(x + width - 112., 385., 112., 36.),
                );
                place(
                    cx,
                    self.transcript,
                    Rect::new(x, 435., width, (h - 517.).max(50.)),
                );
            }
            1 => {
                place(cx, self.search, Rect::new(x, 116., width, 40.));
                for (i, b) in self.rows.iter().enumerate() {
                    if i < self.visible_rows
                        && self
                            .history
                            .get(self.history_page * self.visible_rows + i)
                            .is_some()
                    {
                        show(cx, *b, Rect::new(x, 174. + i as f32 * 42., width, 36.));
                    }
                }
                if self.history.is_empty() {
                    self.caption(
                        cx,
                        if self.search_text.is_empty() {
                            "No dictations yet. Hold your hotkey to start."
                        } else {
                            "No matching dictations."
                        },
                        Point::new(x, 192.),
                        15.,
                        MUTED,
                        width,
                    );
                }
                let footer = 182. + self.visible_rows as f32 * 42.;
                let button_w = (width - 16.) / 3.;
                show(cx, self.actions[9], Rect::new(x, footer, button_w, 34.));
                show(
                    cx,
                    self.actions[10],
                    Rect::new(x + button_w + 8., footer, button_w, 34.),
                );
                show(
                    cx,
                    self.actions[1],
                    Rect::new(x + width - button_w, footer, button_w, 34.),
                );
                place(
                    cx,
                    self.transcript,
                    Rect::new(x, footer + 44., width, (h - footer - 122.).max(30.)),
                );
            }
            2 => {
                let gap = if h < 800. { 42. } else { 46. };
                let label_width = if width < 450. { 148. } else { 185. };
                for (i, (title, _, _)) in FIELDS.iter().enumerate() {
                    let y = 115. + i as f32 * gap - self.settings_scroll;
                    if y < 114. || y + 36. > h - 169. {
                        continue;
                    }
                    self.caption(
                        cx,
                        *title,
                        Point::new(x, y + 10.),
                        13.,
                        MUTED,
                        label_width - 10.,
                    );
                    show(
                        cx,
                        self.fields[i],
                        Rect::new(x + label_width, y, width - label_width, 36.),
                    );
                }
                let y = 115. + FIELDS.len() as f32 * gap - self.settings_scroll;
                if y >= 114. && y + 36. <= h - 169. {
                    self.caption(
                        cx,
                        "Language",
                        Point::new(x, y + 10.),
                        13.,
                        MUTED,
                        label_width - 10.,
                    );
                    place(
                        cx,
                        self.language,
                        Rect::new(x + label_width, y, width - label_width, 36.),
                    );
                }
                let hotkey = self.config["input"]["trigger_key"]
                    .as_str()
                    .unwrap_or("Not loaded");
                let hotkey = key_name(hotkey);
                self.caption(
                    cx,
                    format!("Hotkey: {hotkey}"),
                    Point::new(x, h - 165.),
                    13.,
                    BLUE,
                    width,
                );
                show(
                    cx,
                    self.actions[5],
                    Rect::new(x, h - 142., (width - 12.) / 2., 36.),
                );
                show(
                    cx,
                    self.actions[4],
                    Rect::new(x + (width + 12.) / 2., h - 142., (width - 12.) / 2., 36.),
                );
                if self.test_active {
                    place(
                        cx,
                        self.wave,
                        Rect::new(x + width - 100., h - 154., 100., 20.),
                    );
                }
                show(
                    cx,
                    self.actions[3],
                    Rect::new(x + width - 150., h - 96., 150., 38.),
                );
            }
            _ => {
                let action_w = (width - 16.) / 3.;
                show(cx, self.actions[6], Rect::new(x, 116., action_w, 36.));
                show(
                    cx,
                    self.actions[11],
                    Rect::new(x + 2. * (action_w + 8.), 116., action_w, 36.),
                );
                show(
                    cx,
                    self.actions[7],
                    Rect::new(x + action_w + 8., 116., action_w, 36.),
                );
                place(
                    cx,
                    self.transcript,
                    Rect::new(x, 174., width, (h - 252.).max(50.)),
                );
            }
        }
        if !self.notice.is_empty() {
            self.caption(
                cx,
                self.notice.clone(),
                Point::new(x, h - 52.),
                12.,
                CORAL,
                width - 100.,
            );
            show(
                cx,
                self.actions[8],
                Rect::new(x + width - 96., h - 54., 84., 32.),
            );
        }
        if let Some(menu) = self.menu {
            place(cx, menu, Rect::new(0., 0., w, h));
        }
        Metrics::new(c.max)
    }
    pub(super) fn paint_view(&self, cx: &mut Paint<'_>) {
        cx.painter.rect(cx.bounds, 0., BACK.into());
        cx.painter.rect(
            Rect::new(self.sidebar, 20., 1., (cx.bounds.height - 40.).max(0.)),
            0.,
            Color::hex(0x29313c).into(),
        );
        if self.page == 0 {
            cx.painter.rect(
                Rect::new(self.content.x, 110., self.content.width, 258.),
                14.,
                PANEL.into(),
            );
        }
        for (at, p, color) in &self.labels {
            cx.painter.paragraph(p, *at, (*color).into());
        }
    }
}
