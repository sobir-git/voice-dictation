use super::*;

impl Desktop {
    pub(super) fn layout_view(&mut self, cx: &mut Layout<'_>, c: Constraints) -> Metrics {
        let unit = gap(cx, 1.);
        let padding = unit * 3.;
        let inner = c.inset(padding);
        let width = inner.max.width;
        let content = width.min(990.);
        let x = padding + (width - content) / 2.;
        let natural = Constraints::loose(Size::new(content, f32::INFINITY));
        let flow = Flow::gap(unit * 1.5).align(Align::Stretch);
        let header_lead = column_at(
            cx,
            Point::new(x, padding),
            natural,
            flow,
            &[
                Entry::natural(self.brand),
                Entry::natural(self.nav).aligned(Align::Start),
            ],
        );
        let title_y = padding + header_lead.size.height + unit * 1.5;
        let title = if self.page == 2 {
            row_at(
                cx,
                Point::new(x, title_y),
                natural,
                Flow::default().align(Align::Center),
                &[
                    Entry::natural(self.title),
                    Entry::fill(self.spacer),
                    Entry::natural(self.actions[3]),
                ],
            )
        } else {
            row_at(
                cx,
                Point::new(x, title_y),
                natural,
                Flow::default().align(Align::Center),
                &[Entry::natural(self.title)],
            )
        };
        let subtitle_y = title_y + title.size.height + unit * 1.5;
        let subtitle = column_at(
            cx,
            Point::new(x, subtitle_y),
            natural,
            Flow::default(),
            &[Entry::natural(self.subtitle)],
        );
        let header_height = subtitle_y + subtitle.size.height - padding;
        let top = padding + header_height + unit * 3.;
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
            Point::new(x, bottom),
            Constraints::loose(Size::new(content, footer_height)),
            Flow::gap(unit).align(Align::Center),
            &footer_entries,
        );
        let body = Constraints::loose(Size::new(content, (bottom - top - unit * 2.).max(0.)));
        let origin = Point::new(x, top);
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
                    Point::new(x, y),
                    natural,
                    Flow::gap(unit).align(Align::Center),
                    &actions,
                );
                let y = y + buttons.size.height + unit * 2.;
                let toolbar = row_at(
                    cx,
                    Point::new(x, y),
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
                    Point::new(x, y),
                    Constraints::tight(Size::new(content, (bottom - y - unit * 2.).max(0.))),
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
                let paging_height = cx.measure(self.actions[9], natural).size.height;
                let paging_y = (top + body.max.height - paging_height).max(list_y);
                let limit = paging_y - unit;
                let empty = cx.measure(self.rows[0], natural).size.height;
                let capacity = (((paging_y - unit - list_y) / (empty + unit * 1.5)).floor()
                    as usize)
                    .max(1)
                    .min(self.rows.len());
                let mut y = list_y;
                let mut fitted = 0usize;
                for (i, row) in self.rows.iter().enumerate() {
                    let candidate = i < self.visible_rows
                        && self
                            .history
                            .get(self.history_page * self.visible_rows + i)
                            .is_some();
                    if !candidate {
                        continue;
                    }
                    let metrics = cx.measure(*row, natural);
                    if fitted > 0 && y + metrics.size.height > limit {
                        cx.place(*row, Point::new(x, cx.viewport().height + unit * 4.));
                        continue;
                    }
                    cx.place(*row, Point::new(x, y));
                    y += metrics.size.height + unit * 1.5;
                    fitted += 1;
                }
                self.visible_rows = if fitted > 0 { fitted } else { capacity }.min(self.rows.len());
                row_at(
                    cx,
                    Point::new(x, paging_y),
                    natural,
                    Flow::gap(unit).align(Align::Center),
                    &[
                        Entry::natural(self.actions[9]),
                        Entry::natural(self.actions[10]),
                        Entry::fill(self.spacer),
                    ],
                );
            }
            2 => {
                let viewport_y = top;
                let viewport_height = (bottom - viewport_y - unit).max(0.);
                column_at(
                    cx,
                    Point::new(x, viewport_y),
                    Constraints::tight(Size::new(content, viewport_height)),
                    Flow::default(),
                    &[Entry::fill(self.preferences)],
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
                    Point::new(x, y),
                    Constraints::tight(Size::new(content, (bottom - y - unit * 2.).max(0.))),
                    Flow::default(),
                    &[Entry::fill(self.transcript)],
                );
            }
        }
        Metrics::new(c.max)
    }
}
