use super::*;
use iced::keyboard::key::Named;
use iced::widget::column;

impl App {
    pub(super) fn has_tag_filter(&self) -> bool {
        !self.tag_filter.is_empty() || !self.excluded_tag_filter.is_empty()
    }

    pub(super) fn check_tag_filter(&mut self, id: i64, on: bool, exclude: bool) -> Task<Message> {
        let Some(index) = self.snapshot.tags.iter().position(|tag| tag.id == id) else {
            return Task::none();
        };
        self.tag_filter_focus = index * 2 + usize::from(exclude);
        let (selected, opposite) = if exclude {
            (&mut self.excluded_tag_filter, &mut self.tag_filter)
        } else {
            (&mut self.tag_filter, &mut self.excluded_tag_filter)
        };
        let changed = if on {
            let removed = opposite.remove(&id);
            selected.insert(id) || removed
        } else {
            selected.remove(&id)
        };
        if changed {
            self.reset_tag_filter_results()
        } else {
            Task::none()
        }
    }

    pub(super) fn reset_tag_filter_results(&mut self) -> Task<Message> {
        self.clear_tag_selection(false);
        self.main_pane.page = 0;
        self.main_pane.page_input = None;
        listing::scroll_to_top(false)
    }

    pub(super) fn tag_filter_key(&mut self, key: Named, reverse: bool) -> Task<Message> {
        if !self.tag_filter_open {
            return Task::none();
        }
        let actions = self.snapshot.tags.len() * 2;
        let count = actions + 3;
        match key {
            Named::Tab | Named::ArrowDown | Named::ArrowUp => {
                let backwards = key == Named::ArrowUp || (key == Named::Tab && reverse);
                let step = if backwards { count - 1 } else { 1 };
                self.tag_filter_focus = (self.tag_filter_focus + step) % count;
                if self.tag_filter_focus == actions + 1 && !self.has_tag_filter() {
                    self.tag_filter_focus = (self.tag_filter_focus + step) % count;
                }
            }
            Named::Home => self.tag_filter_focus = 0,
            Named::End => self.tag_filter_focus = count - 1,
            Named::Space | Named::Enter => {
                let message = if let Some(tag) = self.snapshot.tags.get(self.tag_filter_focus / 2) {
                    if self.tag_filter_focus.is_multiple_of(2) {
                        Message::TagFilterCheck(tag.id, !self.tag_filter.contains(&tag.id))
                    } else {
                        Message::ExcludeTagFilterCheck(
                            tag.id,
                            !self.excluded_tag_filter.contains(&tag.id),
                        )
                    }
                } else {
                    match self.tag_filter_focus - actions {
                        0 => Message::ShowTagManagement,
                        1 => Message::ClearTagFilter,
                        _ => Message::TagFilterOpen(false),
                    }
                };
                return self.update(message);
            }
            _ => return Task::none(),
        }
        if let Some(tag) = self.snapshot.tags.get(self.tag_filter_focus / 2) {
            return tags::scroll_tag_into_view(format!("filter-tag-{}", tag.id), "filter-tags");
        }
        Task::none()
    }

    pub(super) fn tag_filter_view(&self) -> Element<'_, Message> {
        let open = self.tag_filter_open && self.dialog.is_none();
        let caption = if !self.has_tag_filter() {
            "我的标签".to_owned()
        } else {
            format!(
                "我的标签 · {}",
                self.tag_filter.len() + self.excluded_tag_filter.len()
            )
        };
        let trigger = button(
            row![
                button_text(caption),
                button_icon(if open { '\u{E70E}' } else { '\u{E70D}' }, 12).width(16),
            ]
            .spacing(8)
            .height(BUTTON_CONTENT_HEIGHT)
            .align_y(Alignment::Center),
        )
        .style(move |theme, status| {
            if open || self.has_tag_filter() {
                item(theme, true, status)
            } else {
                secondary(theme, status)
            }
        })
        .padding(TOOLBAR_CONTROL_PADDING)
        .on_press(Message::TagFilterOpen(!open));
        dropdown::tag_menu(trigger, open.then(|| self.tag_filter_menu()))
    }

    fn tag_filter_menu(&self) -> Element<'_, Message> {
        let colors = self.colors();
        let mut options = column![].spacing(4);
        for (index, tag) in self.snapshot.tags.iter().enumerate() {
            let id = tag.id;
            let choices = [false, true].map(|exclude| {
                let selected = if exclude {
                    self.excluded_tag_filter.contains(&id)
                } else {
                    self.tag_filter.contains(&id)
                };
                let focused = self.tag_filter_focus == index * 2 + usize::from(exclude);
                let message = move |on| {
                    if exclude {
                        Message::ExcludeTagFilterCheck(id, on)
                    } else {
                        Message::TagFilterCheck(id, on)
                    }
                };
                button(
                    row![
                        checkbox(selected).size(18).on_toggle(message),
                        button_text(if exclude { "排除" } else { "包含" }).size(12),
                    ]
                    .spacing(8)
                    .height(BUTTON_CONTENT_HEIGHT)
                    .align_y(Alignment::Center),
                )
                .padding(6)
                .style(move |theme, status| filter_button(theme, status, focused, selected))
                .on_press(message(!selected))
            });
            let [include, exclude] = choices;
            options = options.push(
                container(
                    row![
                        text(&tag.name)
                            .size(13)
                            .width(Fill)
                            .wrapping(text::Wrapping::Glyph),
                        text(self.tag_count(id).to_string())
                            .size(11)
                            .color(colors.muted),
                        include,
                        exclude,
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .id(format!("filter-tag-{id}")),
            );
        }
        if self.snapshot.tags.is_empty() {
            options = options.push(
                text("还没有标签，打开“标签管理”新建标签。")
                    .size(13)
                    .color(colors.muted)
                    .wrapping(text::Wrapping::Glyph),
            );
        }
        let actions = self.snapshot.tags.len() * 2;
        let focus = self.tag_filter_focus;
        container(
            column![
                text("包含：须包含全部所选标签\n排除：不能含有任一所选标签")
                    .size(12)
                    .color(colors.muted),
                scrollable(options)
                    .id("filter-tags")
                    .height(Fill)
                    .direction(scrollable::Direction::Vertical(
                        content_scrollbar().spacing(8)
                    )),
                widget::rule::horizontal(1).style(separator),
                row![
                    button(button_text("标签管理").size(12))
                        .padding(8)
                        .style(move |theme, status| filter_button(
                            theme,
                            status,
                            focus == actions,
                            false
                        ))
                        .on_press(Message::ShowTagManagement),
                    button(button_text("清除筛选").size(12))
                        .padding(8)
                        .style(move |theme, status| filter_button(
                            theme,
                            status,
                            focus == actions + 1,
                            false
                        ))
                        .on_press_maybe(self.has_tag_filter().then_some(Message::ClearTagFilter)),
                    space::horizontal(),
                    button(button_text("完成").size(12))
                        .padding(8)
                        .style(move |theme, status| filter_button(
                            theme,
                            status,
                            focus == actions + 2,
                            false
                        ))
                        .on_press(Message::TagFilterOpen(false)),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            ]
            .spacing(12),
        )
        .padding(12)
        .width(380)
        .height(if self.snapshot.tags.is_empty() {
            170
        } else {
            360
        })
        .style(menu_surface)
        .into()
    }
}

fn filter_button(
    theme: &iced::Theme,
    status: widget::button::Status,
    focused: bool,
    selected: bool,
) -> widget::button::Style {
    button_focus(theme, status, focused, item(theme, selected, status))
}
