use super::*;

impl Desktop {
    pub(super) fn layout_view(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        let unit = gap(cx, 1.);
        let padding = unit * 3.;
        let inner = c.inset(padding);
        let width = inner.max.width;
        let natural = Constraints::loose(Size::new(width, f32::INFINITY));
        let flow = Flow::gap(unit * 1.5).align(Align::Stretch);
        let header = column_at(
            cx,
            Point::new(padding, padding),
            natural,
            flow,
            &[
                Entry::natural(self.brand),
                Entry::natural(self.nav).aligned(Align::Start),
                Entry::natural(self.title),
                Entry::natural(self.subtitle),
            ],
        );
        let top = padding + header.size.height + unit * 3.;
        let footer_entries = if self.notice.is_empty() {
            vec![Entry::fill(self.feedback)]
        } else {
            vec![Entry::fill(self.feedback), Entry::natural(self.actions[8])]
        };
        let footer_height =
            cx.measure(self.feedback, natural)
                .size
                .height
                .max(if self.notice.is_empty() {
                    0.
                } else {
                    cx.measure(self.actions[8], natural).size.height
                });
        let bottom = (c.max.height - padding - footer_height).max(top);
        row_at(
            cx,
            Point::new(padding, bottom),
            Constraints::loose(Size::new(width, footer_height)),
            Flow::gap(unit).align(Align::Center),
            &footer_entries,
        );
        let body = Constraints::loose(Size::new(width, (bottom - top - unit * 2.).max(0.)));
        let origin = Point::new(padding, top);
        match self.page {
            0 => {
                let hero = column_at(cx, origin, body, flow, &[Entry::natural(self.recording)]);
                let y = top + hero.size.height + unit;
                let mut actions = vec![Entry::natural(self.actions[0])];
                if self.state["recording"].as_bool() == Some(true)
                    || self.state["processing"].as_bool() == Some(true)
                {
                    actions.push(Entry::natural(self.actions[2]));
                }
                let buttons = row_at(
                    cx,
                    Point::new(padding, y),
                    natural,
                    Flow::gap(unit).align(Align::Center),
                    &actions,
                );
                let y = y + buttons.size.height + unit * 2.;
                let toolbar = row_at(
                    cx,
                    Point::new(padding, y),
                    natural,
                    Flow::gap(unit).align(Align::Center),
                    &[
                        Entry::natural(self.section),
                        Entry::fill(self.spacer),
                        Entry::natural(self.actions[1]),
                    ],
                );
                let y = y + toolbar.size.height + unit;
                column_at(
                    cx,
                    Point::new(padding, y),
                    Constraints::tight(Size::new(width, (bottom - y - unit * 2.).max(0.))),
                    Flow::default(),
                    &[Entry::fill(self.transcript)],
                );
            }
            1 => {
                let tools = row_at(
                    cx,
                    origin,
                    natural,
                    Flow::gap(unit).align(Align::Center),
                    &[Entry::fill(self.search), Entry::natural(self.actions[12])],
                );
                let list_y = top + tools.size.height + unit * 1.5;
                let mut entries = vec![];
                for (i, row) in self.rows.iter().enumerate() {
                    if i < self.visible_rows
                        && self
                            .history
                            .get(self.history_page * self.visible_rows + i)
                            .is_some()
                    {
                        entries.push(Entry::natural(*row));
                    }
                }
                let list = column_at(
                    cx,
                    Point::new(padding, list_y),
                    body,
                    Flow::gap(unit).align(Align::Stretch),
                    &entries,
                );
                let y = list_y + list.size.height + unit;
                let toolbar = row_at(
                    cx,
                    Point::new(padding, y),
                    natural,
                    Flow::gap(unit).align(Align::Center),
                    &[
                        Entry::natural(self.actions[9]),
                        Entry::natural(self.actions[10]),
                        Entry::fill(self.spacer),
                    ],
                );
                let _ = toolbar;
            }
            2 => {
                let action_height = cx.measure(self.actions[5], natural).size.height;
                let save_height = cx.measure(self.actions[3], natural).size.height;
                let key_height =
                    cx.measure(self.hotkey, natural)
                        .size
                        .height
                        .max(if self.test_active {
                            cx.measure(self.meter, natural).size.height
                        } else {
                            0.
                        });
                let tools_height = action_height + save_height + key_height + unit * 3.;
                let viewport_height = (body.max.height - tools_height - unit * 2.).max(0.);
                column_at(
                    cx,
                    origin,
                    Constraints::tight(Size::new(width, viewport_height)),
                    Flow::default(),
                    &[Entry::fill(self.preferences)],
                );
                let y = top + viewport_height + unit * 2.;
                let mut key_entries = vec![Entry::natural(self.hotkey)];
                if self.test_active {
                    key_entries.push(Entry::fill(self.meter));
                }
                let key = row_at(
                    cx,
                    Point::new(padding, y),
                    natural,
                    Flow::gap(unit * 2.).align(Align::Center),
                    &key_entries,
                );
                let y = y + key.size.height + unit;
                let tools = row_at(
                    cx,
                    Point::new(padding, y),
                    natural,
                    Flow::gap(unit).align(Align::Center),
                    &[
                        Entry::natural(self.actions[5]),
                        Entry::natural(self.actions[4]),
                    ],
                );
                row_at(
                    cx,
                    Point::new(padding, y + tools.size.height + unit),
                    natural,
                    Flow::default().justify(Justify::End),
                    &[Entry::natural(self.actions[3])],
                );
            }
            _ => {
                let toolbar = row_at(
                    cx,
                    origin,
                    natural,
                    Flow::gap(unit).align(Align::Center),
                    &[
                        Entry::natural(self.actions[6]),
                        Entry::natural(self.actions[7]),
                        Entry::natural(self.actions[11]),
                    ],
                );
                let y = top + toolbar.size.height + unit;
                column_at(
                    cx,
                    Point::new(padding, y),
                    Constraints::tight(Size::new(width, (bottom - y - unit * 2.).max(0.))),
                    Flow::default(),
                    &[Entry::fill(self.transcript)],
                );
            }
        }
        Metrics::new(c.max)
    }
}
