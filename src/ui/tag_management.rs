use super::*;
use iced::{keyboard::key::Named, widget::column};

impl App {
    pub(super) fn show_page(&mut self, page: MainPage) {
        self.tag_filter_open = false;
        self.page = page;
        if page != MainPage::Library && self.player_window == self.main {
            // A hidden main-window player must not start or keep playing.
            if self.loader.is_some() {
                self.stop_player();
            } else if let Some(player) = &mut self.player {
                player.set_paused(true);
            }
        }
    }

    pub(super) fn tag_management_enabled(&self) -> bool {
        self.service.is_some() && !self.library_loading && !self.updates.installing()
    }

    pub(super) fn tag_management_key(&mut self, key: Named, reverse: bool) -> Task<Message> {
        let count = if self.tag_management_enabled() {
            self.snapshot.tags.len() * 2 + 2
        } else {
            1 // Returning to the library stays available while data loads.
        };
        self.tag_management_focus = self.tag_management_focus.min(count - 1);
        match key {
            Named::Tab | Named::ArrowDown | Named::ArrowUp => {
                let backwards = key == Named::ArrowUp || (key == Named::Tab && reverse);
                self.tag_management_focus =
                    (self.tag_management_focus + if backwards { count - 1 } else { 1 }) % count;
            }
            Named::Home => self.tag_management_focus = 0,
            Named::End => self.tag_management_focus = count - 1,
            Named::Enter | Named::Space => {
                let message = match self.tag_management_focus {
                    0 => Message::ShowLibrary,
                    1 => Message::NewTag(self.main),
                    focus => {
                        let tag = &self.snapshot.tags[(focus - 2) / 2];
                        if focus % 2 == 0 {
                            Message::EditTag(tag.id)
                        } else {
                            Message::DeleteTag(tag.id)
                        }
                    }
                };
                return self.update(message);
            }
            _ => return Task::none(),
        }
        if self.tag_management_focus >= 2 {
            let tag = &self.snapshot.tags[(self.tag_management_focus - 2) / 2];
            return tags::scroll_tag_into_view(format!("managed-tag-{}", tag.id), "managed-tags");
        }
        Task::none()
    }

    pub(super) fn tag_management_header(&self, compact: bool) -> Element<'_, Message> {
        let focus = self.tag_management_focus;
        let title = column![
            row![
                heading("标签管理", 28),
                views::badge(self.colors(), self.snapshot.tags.len().to_string(), false)
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            text("统一整理标签，方便查找与分类视频")
                .size(13)
                .color(self.colors().muted),
        ]
        .spacing(8)
        .width(Fill);
        let actions = row![
            button(button_text("返回视频列表").size(13))
                .padding([10, 14])
                .style(move |theme, status| management_button(theme, status, focus == 0, secondary))
                .on_press(Message::ShowLibrary),
            button(label(ADD, "新建标签"))
                .padding([10, 16])
                .style(move |theme, status| management_button(theme, status, focus == 1, primary))
                .on_press_maybe(
                    self.tag_management_enabled()
                        .then_some(Message::NewTag(self.main))
                ),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        self.page_header(title.into(), Some(actions.into()), false, compact)
    }

    pub(super) fn tag_management_view(&self) -> Element<'_, Message> {
        let colors = self.colors();
        let enabled = self.tag_management_enabled();
        let body: Element<_> = if self.library_loading {
            container(
                column![
                    motion::spinner(28.0),
                    text("正在读取标签…").size(13).color(colors.muted)
                ]
                .spacing(16)
                .align_x(Alignment::Center),
            )
            .center_x(Fill)
            .center_y(Fill)
            .into()
        } else if self.snapshot.tags.is_empty() {
            container(
                column![
                    icon(TAG, 36).color(colors.accent),
                    heading("还没有标签", 22),
                    text("点击右上角“新建标签”，按主题整理视频。")
                        .size(13)
                        .color(colors.muted),
                ]
                .spacing(16)
                .align_x(Alignment::Center),
            )
            .center_x(Fill)
            .center_y(Fill)
            .into()
        } else {
            let mut rows = column![].spacing(8);
            for (index, tag) in self.snapshot.tags.iter().enumerate() {
                let focus = self.tag_management_focus;
                rows = rows.push(
                    container(
                        row![
                            icon(TAG, 18).color(colors.muted),
                            text(&tag.name)
                                .size(14)
                                .width(Fill)
                                .wrapping(text::Wrapping::Glyph),
                            text(format!("{} 个视频", self.tag_count(tag.id)))
                                .size(12)
                                .color(colors.muted)
                                .width(100),
                            button(button_text("重命名").size(13))
                                .padding([9, 12])
                                .style(move |theme, status| management_button(
                                    theme,
                                    status,
                                    focus == index * 2 + 2,
                                    secondary
                                ))
                                .on_press_maybe(enabled.then_some(Message::EditTag(tag.id))),
                            button(button_text("删除标签").size(13))
                                .padding([9, 12])
                                .style(move |theme, status| management_button(
                                    theme,
                                    status,
                                    focus == index * 2 + 3,
                                    danger
                                ))
                                .on_press_maybe(enabled.then_some(Message::DeleteTag(tag.id))),
                        ]
                        .spacing(12)
                        .align_y(Alignment::Center),
                    )
                    .padding(12)
                    .width(Fill)
                    .style(inset)
                    .id(format!("managed-tag-{}", tag.id)),
                );
            }
            scrollable(rows)
                .id("managed-tags")
                .height(Fill)
                .direction(scrollable::Direction::Vertical(
                    content_scrollbar().spacing(8),
                ))
                .into()
        };
        container(
            column![
                text("视频数量按资料库条目统计，相同内容的副本只计一次。")
                    .size(12)
                    .color(colors.muted),
                body,
                text("Tab / ↑↓ 切换操作，Enter 执行；删除标签不会删除视频文件。")
                    .size(12)
                    .color(colors.muted),
            ]
            .spacing(16),
        )
        .padding(16)
        .width(Fill)
        .height(Fill)
        .style(panel)
        .into()
    }
}

fn management_button(
    theme: &iced::Theme,
    status: widget::button::Status,
    focused: bool,
    base: fn(&iced::Theme, widget::button::Status) -> widget::button::Style,
) -> widget::button::Style {
    button_focus(theme, status, focused, base(theme, status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_navigation_preserves_library_and_pending_state() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(dir.path().into(), None, None);
        app.pending = Some(window::Id::unique());
        app.filter = Filter::Missing;
        app.tag_filter.extend([1, 2]);
        app.tag_filter_open = true;
        app.main_pane.search = "保留搜索".into();
        app.main_pane.page = 3;
        app.main_pane.selected.insert(7);
        app.main_pane.focus = Some(7);
        app.pending_pane.tag_draft.insert(9);
        app.pending_pane.selected.insert(8);

        let _ = app.update(Message::ShowTagManagement);
        assert_eq!(app.page, MainPage::Tags);
        assert!(!app.tag_filter_open);
        let _ = app.update(Message::FocusSearch(app.pending.unwrap()));
        assert_eq!(app.page, MainPage::Tags);
        let _ = app.update(Message::ShowLibrary);
        assert_eq!(app.page, MainPage::Library);
        assert_eq!(app.filter, Filter::Missing);
        assert_eq!(app.tag_filter, BTreeSet::from([1, 2]));
        assert_eq!(app.main_pane.search, "保留搜索");
        assert_eq!(app.main_pane.page, 3);
        assert_eq!(app.main_pane.selected, BTreeSet::from([7]));
        assert_eq!(app.main_pane.focus, Some(7));
        assert_eq!(app.pending_pane.tag_draft, BTreeSet::from([9]));
        assert_eq!(app.pending_pane.selected, BTreeSet::from([8]));

        let _ = app.update(Message::ShowSettings);
        assert_eq!(app.page, MainPage::Settings);
        let _ = app.update(Message::ShowTagManagement);
        let _ = app.update(Message::FocusSearch(app.main));
        assert_eq!(app.page, MainPage::Library);
        let _ = app.update(Message::ShowTagManagement);
        let _ = app.update(Message::Filter(Filter::All));
        assert_eq!(app.page, MainPage::Library);
        assert_eq!(app.filter, Filter::All);
    }

    // Consume only service data; no window tasks or desktop automation run.
    fn snapshot(app: &mut App, ready: impl Fn(&Snapshot) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let timeout = deadline.saturating_duration_since(std::time::Instant::now());
            let event = app
                .service
                .as_ref()
                .unwrap()
                .events
                .recv_timeout(timeout)
                .unwrap();
            match event {
                Event::Snapshot(value) => {
                    app.snapshot = value;
                    app.library_loading = false;
                    app.reconcile_tags();
                    if ready(&app.snapshot) {
                        return;
                    }
                }
                Event::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
    }

    #[test]
    fn management_keyboard_creates_renames_and_confirms_deletion_through_service() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(dir.path().into(), None, None);
        snapshot(&mut app, |_| true);
        let _ = app.update(Message::ShowTagManagement);
        let _ = app.update(Message::NavigationKey(app.main, Named::Enter, false));
        assert!(matches!(app.dialog, Some((_, Dialog::Tag(None)))));
        let _ = app.update(Message::TagName(" ".into()));
        let _ = app.update(Message::SaveTag);
        assert!(app.dialog.is_some());
        let _ = app.update(Message::TagName("旅行".into()));
        let _ = app.update(Message::SaveTag);
        snapshot(&mut app, |s| s.tags.len() == 1);
        let tag_id = app.snapshot.tags[0].id;
        app.tag_filter.insert(tag_id);

        let _ = app.update(Message::NavigationKey(app.main, Named::ArrowDown, false));
        let _ = app.update(Message::NavigationKey(app.main, Named::Space, false));
        assert!(matches!(app.dialog, Some((_, Dialog::Tag(Some(id)))) if id == tag_id));
        assert_eq!(app.tag_name, "旅行");
        let _ = app.update(Message::TagName("旅行记录".into()));
        let _ = app.update(Message::SaveTag);
        snapshot(&mut app, |s| s.tags[0].name == "旅行记录");
        assert_eq!(app.tag_filter, BTreeSet::from([tag_id]));

        let _ = app.update(Message::NavigationKey(app.main, Named::End, false));
        let _ = app.update(Message::NavigationKey(app.main, Named::Enter, false));
        assert!(matches!(app.dialog, Some((_, Dialog::DeleteTag(id))) if id == tag_id));
        let _ = app.update(Message::Dismiss);
        assert_eq!(app.snapshot.tags.len(), 1);
        let _ = app.update(Message::NavigationKey(app.main, Named::Enter, false));
        let _ = app.update(Message::ConfirmDeleteTag);
        snapshot(&mut app, |s| s.tags.is_empty());
        assert!(app.tag_filter.is_empty());
        assert_eq!(app.page, MainPage::Tags);
        assert_eq!(app.tag_management_focus, 1);
    }

    #[test]
    fn unavailable_management_skips_disabled_actions() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(dir.path().into(), None, None);
        app.library_loading = true;
        let _ = app.update(Message::ShowTagManagement);
        assert!(!app.tag_management_enabled());
        let _ = app.update(Message::NavigationKey(app.main, Named::End, false));
        assert_eq!(app.tag_management_focus, 0);
        let _ = app.update(Message::NavigationKey(app.main, Named::Enter, false));
        assert_eq!(app.page, MainPage::Library);
        assert!(app.dialog.is_none());
    }
}
