//! Stable list commands and their anchored overflow menu.
use super::*;
use iced::{keyboard::key::Named, widget::column};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    RemoveTags,
    ManageTags,
    Forget,
    Trash,
}

fn menu_actions(pending: bool, compact: bool) -> &'static [Action] {
    if !pending && compact {
        &[
            Action::RemoveTags,
            Action::ManageTags,
            Action::Forget,
            Action::Trash,
        ]
    } else {
        &[Action::ManageTags, Action::Forget, Action::Trash]
    }
}

pub(super) fn menu_id(pending: bool) -> &'static str {
    if pending {
        "pending-commands"
    } else {
        "library-commands"
    }
}

impl App {
    fn selection_enabled(&self, pending: bool) -> bool {
        !self.library_loading && !self.ids(pending).is_empty()
    }

    pub(super) fn open_command_menu(&mut self, pending: bool, open: bool) -> Task<Message> {
        let open = open && self.selection_enabled(pending) && self.dialog.is_none();
        let pane = self.pane_mut(pending);
        pane.command_menu_open = open;
        if open {
            pane.command_menu_focus = None;
            self.tag_filter_open = false;
            return widget::operation::focus(menu_id(pending));
        }
        Task::none()
    }

    pub(super) fn command_menu_key(
        &mut self,
        pending: bool,
        compact: bool,
        key: Named,
        reverse: bool,
    ) -> Task<Message> {
        if !self.pane(pending).command_menu_open {
            return Task::none();
        }
        let actions = menu_actions(pending, compact);
        let pane = self.pane_mut(pending);
        let focus = actions
            .iter()
            .position(|action| Some(*action) == pane.command_menu_focus)
            .unwrap_or(0);
        match key {
            Named::Tab | Named::ArrowDown | Named::ArrowUp => {
                let backwards = key == Named::ArrowUp || (key == Named::Tab && reverse);
                let next = (focus + if backwards { actions.len() - 1 } else { 1 }) % actions.len();
                pane.command_menu_focus = Some(actions[next]);
            }
            Named::Home => pane.command_menu_focus = Some(actions[0]),
            Named::End => pane.command_menu_focus = actions.last().copied(),
            Named::Enter | Named::Space => {
                let action = actions[focus];
                return self.selection_command(pending, action);
            }
            Named::Escape => pane.command_menu_open = false,
            _ => {}
        }
        Task::none()
    }

    pub(super) fn selection_command(&mut self, pending: bool, action: Action) -> Task<Message> {
        self.pane_mut(pending).command_menu_open = false;
        if !self.selection_enabled(pending) || self.dialog.is_some() {
            return Task::none();
        }
        let owner = if pending {
            let Some(id) = self.pending else {
                return Task::none();
            };
            id
        } else {
            self.main
        };
        self.update(match action {
            Action::RemoveTags if !pending => Message::PickTags(TagAction::Remove),
            Action::RemoveTags => return Task::none(),
            Action::ManageTags => Message::ShowTagManagement,
            Action::Forget => Message::Forget(pending),
            Action::Trash => Message::Trash(pending, owner),
        })
    }

    pub(super) fn selection_commands(&self, pending: bool, compact: bool) -> Element<'_, Message> {
        let colors = self.colors();
        let pane = self.pane(pending);
        let count = self.ids(pending).len();
        let enabled = count > 0 && !self.library_loading;
        let open = enabled && pane.command_menu_open && self.dialog.is_none();
        let summary = if count == 0 {
            "选择视频后可操作".to_owned()
        } else {
            format!("已选 {count} 项")
        };
        // Keep the same command row in idle and selected states. Neither the
        // list viewport nor the footer moves when the selection changes.
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
            commands = commands.push(
                button(label(TAG, "添加标签"))
                    .padding([6, 8])
                    .style(secondary)
                    .on_press_maybe(enabled.then_some(Message::PickTags(TagAction::Add))),
            );
            if !compact {
                commands = commands.push(
                    button(button_text("移除标签").size(12))
                        .padding([6, 8])
                        .style(quiet)
                        .on_press_maybe(enabled.then_some(Message::PickTags(TagAction::Remove))),
                );
            }
        }
        let more = button(
            row![
                button_icon(MORE, 16).width(16),
                button_text("更多").size(12)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([6, 8])
        .style(move |theme, status| item(theme, open, status))
        .on_press_maybe(enabled.then_some(Message::CommandMenu(pending, !open)));
        commands = commands.push(dropdown::command_menu(
            more,
            open.then(|| self.selection_menu(pending, compact)),
            pending,
            compact,
            enabled,
        ));
        container(commands).padding([8, 16]).width(Fill).into()
    }

    fn selection_menu(&self, pending: bool, compact: bool) -> Element<'_, Message> {
        let actions = menu_actions(pending, compact);
        let focus = self
            .pane(pending)
            .command_menu_focus
            .filter(|action| actions.contains(action))
            .unwrap_or(actions[0]);
        let mut options = column![].spacing(4);
        for &action in actions {
            let (glyph, caption, description) = match action {
                Action::RemoveTags => (TAG, "移除标签", None),
                Action::ManageTags => (TAG, "标签管理", None),
                Action::Forget => (
                    REMOVE,
                    if pending {
                        "忽略所选"
                    } else {
                        "从资料库移除"
                    },
                    Some("保留原文件"),
                ),
                Action::Trash => (DELETE, "移入回收站…", Some("下一步选择副本并确认路径")),
            };
            if action == Action::Forget {
                options = options
                    .push(container(widget::rule::horizontal(1).style(separator)).padding([4, 0]));
            }
            let mut content = column![label(glyph, caption)].spacing(4);
            if let Some(description) = description {
                content = content.push(
                    container(text(description).size(11).color(self.colors().muted))
                        .padding(iced::Padding::ZERO.left(24)),
                );
            }
            options = options.push(
                button(content)
                    .width(Fill)
                    .padding(8)
                    .style(move |theme, status| {
                        let style = if action == Action::Trash {
                            danger(theme, status)
                        } else {
                            quiet(theme, status)
                        };
                        button_focus(theme, status, focus == action, style)
                    })
                    .on_press(Message::SelectionCommand(pending, action)),
            );
        }
        container(options)
            .padding(8)
            .width(240)
            .style(menu_surface)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (tempfile::TempDir, App) {
        let temp = tempfile::tempdir().unwrap();
        // Discard the window task and stop the service before supplying fixtures.
        let (mut app, _) = App::boot(temp.path().to_path_buf(), None, None);
        if let Some(mut service) = app.service.take() {
            service.shutdown();
        }
        app.library_loading = false;
        app.pending = Some(window::Id::unique());
        app.snapshot.videos = (1..=3)
            .map(|id| VideoItem {
                id,
                hash: format!("{id:064x}"),
                title: format!("视频 {id}"),
                tag_ids: vec![1],
                library: id != 3,
                created_at: id,
                duration_ms: None,
                files: (0..2)
                    .map(|copy| VideoFile {
                        id: id * 10 + copy,
                        video_id: id,
                        path: format!("C:\\菜单测试\\视频 {id} 副本 {copy}.mp4").into(),
                        size: 10,
                        modified: 100,
                        state: if id == 3 {
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
    fn compact_menu_keyboard_reaches_tag_commands_and_preserves_selection() {
        let (_temp, mut app) = app();
        app.main_pane.selected.extend([1, 2]);
        app.main_pane.page = 4;
        let _ = app.update(Message::CommandMenu(false, true));
        let _ = app.update(Message::CommandMenuKey(false, true, Named::Tab, false));
        assert_eq!(app.main_pane.command_menu_focus, Some(Action::ManageTags));
        // Expanding the list must keep focus on the same command as options move.
        let _ = app.update(Message::CommandMenuKey(false, false, Named::Enter, false));
        assert_eq!(app.page, MainPage::Tags);
        assert!(!app.main_pane.command_menu_open);
        assert_eq!(app.main_pane.selected, BTreeSet::from([1, 2]));
        assert_eq!(app.main_pane.page, 4);
        let _ = app.update(Message::ShowLibrary);
        let _ = app.update(Message::CommandMenu(false, true));
        let _ = app.update(Message::CommandMenuKey(false, true, Named::Tab, true));
        assert_eq!(app.main_pane.command_menu_focus, Some(Action::Trash));
        let _ = app.update(Message::CommandMenuKey(false, true, Named::Home, false));
        let _ = app.update(Message::CommandMenuKey(false, true, Named::Space, false));
        let Some((owner, Dialog::Tags(picker))) = &app.dialog else {
            panic!("expected the tag picker");
        };
        assert_eq!(*owner, app.main);
        assert_eq!(picker.action, TagAction::Remove);
        assert_eq!(picker.ids, [1, 2]);
    }

    #[test]
    fn trash_menu_opens_path_confirmation_for_its_window_without_removing_files() {
        for pending in [false, true] {
            let (_temp, mut app) = app();
            app.main_pane.selected.extend([1, 2]);
            app.pending_pane.selected.insert(3);
            let _ = app.update(Message::CommandMenu(pending, true));
            let _ = app.update(Message::CommandMenuKey(pending, true, Named::End, false));
            let _ = app.update(Message::CommandMenuKey(pending, true, Named::Enter, false));
            let Some((
                owner,
                Dialog::Trash {
                    files,
                    selected,
                    targets,
                },
            )) = &app.dialog
            else {
                panic!("expected path confirmation before any file operation");
            };
            assert_eq!(
                *owner,
                if pending {
                    app.pending.unwrap()
                } else {
                    app.main
                }
            );
            let ids = if pending { vec![3] } else { vec![1, 2] };
            assert_eq!(files.len(), ids.len() * 2);
            assert!(files.iter().all(|file| ids.contains(&file.video_id)));
            assert_eq!(selected.len(), ids.len());
            assert_eq!(targets.len(), files.len());
            assert!(
                targets
                    .iter()
                    .all(|target| files.iter().any(|file| file.path == target.path))
            );
            assert!(!app.pane(pending).command_menu_open);
            let _ = app.update(Message::Dismiss);
            assert_eq!(app.main_pane.selected, BTreeSet::from([1, 2]));
            assert_eq!(app.pending_pane.selected, BTreeSet::from([3]));
            assert_eq!(app.snapshot.videos.len(), 3);
        }
    }

    #[test]
    fn menu_dismissal_and_disabled_states_do_not_execute_or_restore_stale_commands() {
        let (_temp, mut app) = app();
        let _ = app.update(Message::CommandMenu(false, true));
        assert!(!app.main_pane.command_menu_open);
        let _ = app.update(Message::SelectionCommand(false, Action::Trash));
        assert!(app.dialog.is_none());
        // A row focus remains a valid single-video context, as before.
        app.main_pane.focus = Some(1);
        let _ = app.update(Message::TagFilterOpen(true));
        let _ = app.update(Message::CommandMenu(false, true));
        assert!(!app.tag_filter_open);
        assert!(app.main_pane.command_menu_open);
        let _ = app.update(Message::CommandMenuKey(false, true, Named::Escape, false));
        assert!(!app.main_pane.command_menu_open);
        assert_eq!(app.ids(false), [1]);
        let _ = app.update(Message::CommandMenu(false, true));
        let _ = app.update(Message::FocusSearch(app.main));
        assert!(!app.main_pane.command_menu_open);
        assert_eq!(app.ids(false), [1]);
        let _ = app.update(Message::CommandMenu(false, true));
        let _ = app.update(Message::ClearSelection(false));
        let _ = app.update(Message::CommandMenuKey(false, true, Named::Enter, false));
        assert!(!app.main_pane.command_menu_open);
        assert!(app.dialog.is_none());
        app.library_loading = true;
        app.main_pane.selected.insert(1);
        let _ = app.update(Message::CommandMenu(false, true));
        assert!(!app.main_pane.command_menu_open);
        let _ = app.update(Message::SelectionCommand(false, Action::Trash));
        assert!(app.dialog.is_none());
    }
}
