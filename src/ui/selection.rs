//! Batch tag commands and single-video context commands.
use super::*;
use iced::{keyboard::key::Named, widget::column};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Forget,
    Delete,
}

const ACTIONS: &[Action] = &[Action::Forget, Action::Delete];

pub(super) fn menu_id(pending: bool, video_id: i64) -> String {
    format!("video-menu-{pending}-{video_id}")
}

impl App {
    pub(super) fn video_list_enabled(&self, pending: bool) -> bool {
        let window = if pending {
            self.pending
        } else {
            Some(self.main)
        };
        // Ordinary dialogs block only their owning window. Folder migration
        // still blocks both windows, matching the modal shown by App::view.
        !self.library_loading
            && !self.migration.busy()
            && window.is_some_and(|window| {
                self.dialog
                    .as_ref()
                    .is_none_or(|(owner, _)| *owner != window)
            })
    }

    pub(super) fn video_command_enabled(&self, pending: bool, id: i64) -> bool {
        self.video_list_enabled(pending)
            && (pending || self.page == MainPage::Library)
            && self.filtered(pending).iter().any(|video| video.id == id)
    }

    pub(super) fn open_context_menu(&mut self, pending: bool, id: Option<i64>) -> Task<Message> {
        let id = id.filter(|&id| self.video_command_enabled(pending, id));
        self.pane_mut(pending).context_menu = id;
        self.pane_mut(pending).context_menu_focus = None;
        if let Some(id) = id {
            self.tag_filter_open = false;
            return widget::operation::focus(menu_id(pending, id));
        }
        Task::none()
    }

    pub(super) fn context_menu_key(
        &mut self,
        pending: bool,
        key: Named,
        reverse: bool,
    ) -> Task<Message> {
        let Some(id) = self.pane(pending).context_menu else {
            return Task::none();
        };
        let pane = self.pane_mut(pending);
        let focus = ACTIONS
            .iter()
            .position(|action| Some(*action) == pane.context_menu_focus)
            .unwrap_or(0);
        match key {
            Named::Tab | Named::ArrowDown | Named::ArrowUp => {
                let backwards = key == Named::ArrowUp || (key == Named::Tab && reverse);
                let next = (focus + if backwards { ACTIONS.len() - 1 } else { 1 }) % ACTIONS.len();
                pane.context_menu_focus = Some(ACTIONS[next]);
            }
            Named::Home => pane.context_menu_focus = Some(ACTIONS[0]),
            Named::End => pane.context_menu_focus = ACTIONS.last().copied(),
            Named::Enter | Named::Space => return self.video_command(pending, id, ACTIONS[focus]),
            Named::Escape => pane.context_menu = None,
            _ => {}
        }
        Task::none()
    }

    pub(super) fn video_command(
        &mut self,
        pending: bool,
        id: i64,
        action: Action,
    ) -> Task<Message> {
        // Require a live row context; checked videos never supply destructive targets.
        let open = self.pane(pending).context_menu == Some(id);
        self.pane_mut(pending).context_menu = None;
        if !open || self.deletion.busy() || !self.video_command_enabled(pending, id) {
            return Task::none();
        }
        match action {
            Action::Forget => {
                self.send(Command::Forget(id, pending));
                let pane = self.pane_mut(pending);
                pane.selected.remove(&id);
                if pane.focus == Some(id) {
                    pane.focus = None;
                }
                pane.tag_draft.clear();
            }
            Action::Delete => return self.update(Message::Delete(pending, id)),
        }
        Task::none()
    }

    pub(super) fn selection_commands(&self, pending: bool, compact: bool) -> Element<'_, Message> {
        let colors = self.colors();
        let pane = self.pane(pending);
        let count = self.ids(pending).len();
        let enabled = count > 0 && !self.library_loading;
        let summary = if count == 0 {
            "选择视频后可操作".to_owned()
        } else {
            format!("已选 {count} 项")
        };
        // Keep the command row height stable as selection changes.
        let clear: Element<_> = if compact {
            button(button_icon(CLOSE, 12))
                .padding([6, 6])
                .style(quiet)
                .on_press_maybe(enabled.then_some(Message::ClearSelection(pending)))
                .into()
        } else {
            button(button_text("取消选择").size(12))
                .padding([6, 8])
                .style(quiet)
                .on_press_maybe(enabled.then_some(Message::ClearSelection(pending)))
                .into()
        };
        let mut commands = row![
            text(summary)
                .size(12)
                .color(if enabled { colors.accent } else { colors.muted })
                .width(Fill),
            clear,
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if pending {
            let caption = if pane.tag_draft.is_empty() {
                "选择标签".to_owned()
            } else if compact {
                format!("标签 · {}", pane.tag_draft.len())
            } else {
                format!("已选 {} 个标签", pane.tag_draft.len())
            };
            commands = commands
                .push(
                    button(button_text(caption).size(12))
                        .padding([6, 8])
                        .style(secondary)
                        .on_press_maybe(enabled.then_some(Message::PickTags(TagAction::Import))),
                )
                .push(
                    button(button_text("导入资料库").size(12))
                        .padding([6, 8])
                        .style(primary)
                        .on_press_maybe(enabled.then_some(Message::Assign(true))),
                );
        } else {
            commands = commands
                .push(
                    button(label(TAG, "添加标签"))
                        .padding([6, 8])
                        .style(secondary)
                        .on_press_maybe(enabled.then_some(Message::PickTags(TagAction::Add))),
                )
                .push(
                    button(button_text("移除标签").size(12))
                        .padding([6, 8])
                        .style(quiet)
                        .on_press_maybe(enabled.then_some(Message::PickTags(TagAction::Remove))),
                );
        }
        container(commands).padding([8, 16]).width(Fill).into()
    }

    pub(super) fn video_menu(&self, pending: bool, id: i64) -> Option<Element<'_, Message>> {
        if self.pane(pending).context_menu != Some(id) || !self.video_command_enabled(pending, id) {
            return None;
        }
        let focus = self.pane(pending).context_menu_focus.unwrap_or(ACTIONS[0]);
        let mut options = column![].spacing(4);
        for &action in ACTIONS {
            let (glyph, caption, description) = match action {
                Action::Forget => (
                    REMOVE,
                    if pending {
                        "忽略此视频"
                    } else {
                        "从资料库移除"
                    },
                    "保留原文件",
                ),
                Action::Delete => (DELETE, "直接删除…", "永久删除所选副本，删除后无法恢复"),
            };
            options = options.push(
                button(
                    column![
                        label(glyph, caption),
                        container(text(description).size(11).color(self.colors().muted))
                            .padding(iced::Padding::ZERO.left(24)),
                    ]
                    .spacing(4),
                )
                .width(Fill)
                .padding(8)
                .style(move |theme, status| {
                    let style = if action == Action::Delete {
                        danger(theme, status)
                    } else {
                        quiet(theme, status)
                    };
                    button_focus(theme, status, focus == action, style)
                })
                .on_press_maybe(
                    (!self.deletion.busy()).then_some(Message::VideoCommand(pending, id, action)),
                ),
            );
        }
        Some(
            container(options)
                .padding(8)
                .width(260)
                .style(menu_surface)
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (tempfile::TempDir, App) {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(temp.path().to_path_buf(), None, None);
        if let Some(mut service) = app.service.take() {
            service.shutdown();
        }
        app.library_loading = false;
        app.pending = Some(window::Id::unique());
        app.snapshot.videos = (1..=4)
            .map(|id| VideoItem {
                id,
                hash: format!("{id:064x}"),
                title: format!("视频 {id}"),
                tag_ids: vec![1],
                library: id <= 2,
                created_at: id,
                duration_ms: None,
                files: (0..2)
                    .map(|copy| VideoFile {
                        id: id * 10 + copy,
                        video_id: id,
                        path: format!("C:\\菜单测试\\视频 {id} 副本 {copy}.mp4").into(),
                        size: 10,
                        modified: 100,
                        state: if id > 2 {
                            FileState::Pending
                        } else {
                            FileState::Managed
                        },
                        availability: Availability::Present,
                    })
                    .collect(),
            })
            .collect();
        (temp, app)
    }

    #[test]
    fn dialogs_block_only_their_window_but_migration_and_loading_block_both_lists() {
        let (_temp, mut app) = app();
        for pending in [false, true] {
            let owner = if pending {
                app.pending.unwrap()
            } else {
                app.main
            };
            app.dialog = Some((owner, Dialog::Tag(None)));
            for (list, target) in [(false, 1), (true, 3)] {
                assert_eq!(app.video_list_enabled(list), list != pending);
                assert_eq!(app.video_command_enabled(list, target), list != pending);
                let _ = app.update(Message::ContextMenu(list, Some(target)));
                assert_eq!(
                    app.pane(list).context_menu,
                    (list != pending).then_some(target),
                );
            }
        }
        app.dialog = Some((app.main, Dialog::Roots));
        app.migration.progress = Some(video_management::migration::Progress::preparing());
        for pending in [false, true] {
            assert!(!app.video_list_enabled(pending));
        }
        app.migration.progress = None;
        app.dialog = None;
        app.library_loading = true;
        for pending in [false, true] {
            assert!(!app.video_list_enabled(pending));
        }
        app.library_loading = false;
        app.pending = None;
        assert!(app.video_list_enabled(false));
        assert!(!app.video_list_enabled(true));
    }

    #[test]
    fn unselected_row_opens_a_context_without_creating_a_selection_or_preview() {
        for pending in [false, true] {
            let (_temp, mut app) = app();
            let target = if pending { 3 } else { 1 };
            app.details_visible[usize::from(pending)] = false;
            let _ = app.update(Message::ContextMenu(pending, Some(target)));
            assert_eq!(app.pane(pending).context_menu, Some(target));
            assert!(app.pane(pending).selected.is_empty());
            assert!(app.pane(pending).focus.is_none());
            assert!(!app.details_visible[usize::from(pending)]);
            assert!(app.player.is_none());
            assert!(app.loader.is_none());
            assert!(app.playing_file.is_none());
        }
    }

    #[test]
    fn context_menu_preserves_details_selection_and_playback_without_loading() {
        for pending in [false, true] {
            let (_temp, mut app) = app();
            let (focus, target) = if pending { (3, 4) } else { (1, 2) };
            let pane = app.pane_mut(pending);
            pane.selected.extend([focus, target]);
            pane.focus = Some(focus);
            pane.tag_draft.insert(1);
            app.details_visible[usize::from(pending)] = false;
            app.playing_file = Some(focus * 10);
            app.position = 42.0;
            let owner = app.player_window;
            let _ = app.update(Message::ContextMenu(pending, Some(target)));
            assert_eq!(app.pane(pending).context_menu, Some(target));
            assert_eq!(app.pane(pending).focus, Some(focus));
            assert_eq!(app.pane(pending).selected, BTreeSet::from([focus, target]));
            assert_eq!(app.pane(pending).tag_draft, BTreeSet::from([1]));
            assert!(!app.details_visible[usize::from(pending)]);
            assert!(app.player.is_none());
            assert!(app.loader.is_none());
            assert_eq!(app.playing_file, Some(focus * 10));
            assert_eq!(app.player_window, owner);
            assert_eq!(app.position, 42.0);
        }
    }

    #[test]
    fn delete_uses_only_right_clicked_video_and_confirms_paths_in_its_window() {
        for pending in [false, true] {
            let (_temp, mut app) = app();
            app.main_pane.selected.extend([1, 2]);
            app.pending_pane.selected.extend([3, 4]);
            let target = if pending { 4 } else { 2 };
            let _ = app.update(Message::ContextMenu(pending, Some(target)));
            let _ = app.update(Message::ContextMenuKey(pending, Named::End, false));
            let _ = app.update(Message::ContextMenuKey(pending, Named::Enter, false));
            let Some((
                owner,
                Dialog::Delete {
                    video_id,
                    files,
                    selected,
                    targets,
                },
            )) = &app.dialog
            else {
                panic!("expected path confirmation");
            };
            assert_eq!(
                *owner,
                if pending {
                    app.pending.unwrap()
                } else {
                    app.main
                }
            );
            assert_eq!(*video_id, target);
            assert_eq!(files.len(), 2);
            assert!(files.iter().all(|file| file.video_id == target));
            assert_eq!(selected.len(), 1);
            assert_eq!(targets.len(), 2);
            assert!(app.pane(pending).context_menu.is_none());
            let _ = app.update(Message::Dismiss);
            assert_eq!(app.main_pane.selected, BTreeSet::from([1, 2]));
            assert_eq!(app.pending_pane.selected, BTreeSet::from([3, 4]));
            assert_eq!(app.snapshot.videos.len(), 4);
        }
    }

    #[test]
    fn stale_disabled_and_dismissed_contexts_cannot_execute_commands() {
        let (_temp, mut app) = app();
        app.main_pane.selected.extend([1, 2]);
        let _ = app.update(Message::VideoCommand(false, 2, Action::Delete));
        assert!(app.dialog.is_none());
        let _ = app.update(Message::ContextMenu(false, Some(2)));
        let _ = app.update(Message::VideoCommand(false, 1, Action::Delete));
        assert!(app.dialog.is_none());
        let _ = app.update(Message::ContextMenu(false, Some(2)));
        let _ = app.update(Message::ContextMenuKey(false, Named::Escape, false));
        let _ = app.update(Message::ContextMenuKey(false, Named::Enter, false));
        assert!(app.dialog.is_none());
        let _ = app.update(Message::ContextMenu(false, Some(2)));
        let _ = app.update(Message::FocusSearch(app.main));
        assert!(app.main_pane.context_menu.is_none());
        app.library_loading = true;
        let _ = app.update(Message::ContextMenu(false, Some(2)));
        assert!(app.main_pane.context_menu.is_none());
        assert_eq!(app.main_pane.selected, BTreeSet::from([1, 2]));
    }

    #[test]
    fn forgetting_one_video_preserves_other_checked_videos_and_detail_focus() {
        let (_temp, mut app) = app();
        app.main_pane.selected.extend([1, 2]);
        app.main_pane.focus = Some(1);
        let _ = app.update(Message::ContextMenu(false, Some(2)));
        let _ = app.update(Message::ContextMenuKey(false, Named::Home, false));
        let _ = app.update(Message::ContextMenuKey(false, Named::Space, false));
        assert_eq!(app.main_pane.selected, BTreeSet::from([1]));
        assert_eq!(app.main_pane.focus, Some(1));
        assert!(app.main_pane.context_menu.is_none());
    }
}
