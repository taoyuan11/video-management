use super::*;
use std::{thread::JoinHandle, time::Instant};
use video_management::migration::{Outcome, Progress};

#[derive(Default)]
pub(super) struct MigrationUi {
    pub source: Option<PathBuf>,
    pub parent: Option<PathBuf>,
    pub destination: Option<PathBuf>,
    pub selecting: bool,
    pub progress: Option<Progress>,
    pub outcome: Option<Outcome>,
    pub closing: bool,
    focus: usize,
    roots_focus: usize,
    pending: Option<Command>,
    releasing: Option<JoinHandle<covers::Covers>>,
    started: Option<Instant>,
    cancelled: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum RootAction {
    Migrate(PathBuf),
    Stop(PathBuf),
    Result,
    Add,
    Done,
}
impl RootAction {
    fn message(&self) -> Message {
        match self {
            Self::Migrate(path) => Message::PickMigration(path.clone()),
            Self::Stop(path) => Message::RemoveRoot(path.clone()),
            Self::Result => Message::ShowMigration,
            Self::Add => Message::PickRoots,
            Self::Done => Message::Dismiss,
        }
    }
}

impl MigrationUi {
    pub fn busy(&self) -> bool {
        self.progress.is_some() || self.releasing.is_some()
    }
    pub fn retry(&self) -> bool {
        self.outcome.as_ref().is_some_and(|o| o.retry)
    }
    pub fn service_stopped(&mut self) {
        if self.busy() {
            self.pending = None;
            self.progress = None;
            self.outcome = Some(Outcome {
                message: "后台服务已停止，请重新启动软件恢复迁移。".into(),
                retry: true,
            });
        }
    }
}
impl Drop for MigrationUi {
    fn drop(&mut self) {
        if let Some(worker) = self.releasing.take() {
            let _ = worker.join();
        }
    }
}

impl App {
    fn root_actions(&self) -> Vec<RootAction> {
        let mut actions = Vec::new();
        for root in &self.snapshot.roots {
            if self.service.is_some() && !self.migration.retry() {
                if !self.migration.selecting && !self.updates.installing() {
                    actions.push(RootAction::Migrate(root.clone()));
                }
                actions.push(RootAction::Stop(root.clone()));
            }
        }
        if self.migration.retry() {
            actions.push(RootAction::Result);
        }
        if self.service.is_some() && !self.migration.retry() {
            actions.push(RootAction::Add);
        }
        actions.push(RootAction::Done);
        actions
    }
    pub(super) fn root_focused(&self, action: &RootAction) -> bool {
        let actions = self.root_actions();
        actions.get(
            self.migration
                .roots_focus
                .min(actions.len().saturating_sub(1)),
        ) == Some(action)
    }
    pub(super) fn roots_key(
        &mut self,
        key: iced::keyboard::key::Named,
        reverse: bool,
    ) -> Task<Message> {
        use iced::keyboard::key::Named;
        let actions = self.root_actions();
        let count = actions.len();
        self.migration.roots_focus = self.migration.roots_focus.min(count - 1);
        match key {
            Named::Tab | Named::ArrowDown | Named::ArrowUp => {
                let back = key == Named::ArrowUp || (key == Named::Tab && reverse);
                self.migration.roots_focus =
                    (self.migration.roots_focus + if back { count - 1 } else { 1 }) % count;
            }
            Named::Home => self.migration.roots_focus = 0,
            Named::End => self.migration.roots_focus = count - 1,
            Named::Enter | Named::Space => {
                return self.update(actions[self.migration.roots_focus].message());
            }
            _ => return Task::none(),
        }
        if let RootAction::Migrate(path) | RootAction::Stop(path) =
            &actions[self.migration.roots_focus]
            && let Some(index) = self.snapshot.roots.iter().position(|root| root == path)
        {
            return tags::scroll_tag_into_view(
                format!("monitored-root-{index}"),
                "monitored-roots",
            );
        }
        if matches!(actions[self.migration.roots_focus], RootAction::Result) {
            return tags::scroll_tag_into_view("migration-result".into(), "monitored-roots");
        }
        Task::none()
    }
    fn migration_actions(&self) -> Vec<(&'static str, Message, bool)> {
        let state = &self.migration;
        if let Some(progress) = &state.progress {
            return if progress.cancellable {
                vec![("取消迁移", Message::CancelMigration, false)]
            } else {
                vec![]
            };
        }
        if state.selecting {
            return vec![];
        }
        if let Some(outcome) = &state.outcome {
            let mut actions = vec![("返回", Message::Dismiss, false)];
            if outcome.retry {
                if self.service.is_some() {
                    actions.push(("重试清理", Message::RetryMigration, true));
                }
            } else if let Some(source) = &state.source
                && self.snapshot.roots.contains(source)
            {
                actions.push(("重新选择目录", Message::PickMigration(source.clone()), true));
            }
            return actions;
        }
        let mut actions = vec![("取消", Message::Dismiss, false)];
        if state.destination.is_some() && self.pending_delete.is_none() && !self.deletion.busy() {
            actions.push(("开始迁移", Message::ConfirmMigration, true));
        }
        actions
    }

    pub(super) fn migration_key(
        &mut self,
        key: iced::keyboard::key::Named,
        reverse: bool,
    ) -> Task<Message> {
        use iced::keyboard::key::Named;
        let actions = self.migration_actions();
        if actions.is_empty() {
            return Task::none();
        }
        let count = actions.len();
        self.migration.focus = self.migration.focus.min(count - 1);
        match key {
            Named::Tab | Named::ArrowDown | Named::ArrowUp => {
                let back = key == Named::ArrowUp || (key == Named::Tab && reverse);
                self.migration.focus =
                    (self.migration.focus + if back { count - 1 } else { 1 }) % count;
            }
            Named::Home => self.migration.focus = 0,
            Named::End => self.migration.focus = count - 1,
            Named::Enter | Named::Space => {
                return self.update(actions[self.migration.focus].1.clone());
            }
            _ => {}
        }
        Task::none()
    }

    pub(super) fn pick_migration(&mut self, source: PathBuf) -> Task<Message> {
        if self.migration.busy()
            || self.migration.retry()
            || self.migration.selecting
            || self.updates.installing()
            || self.service.is_none()
        {
            return Task::none();
        }
        self.migration.source = Some(source.clone());
        self.migration.destination = None;
        self.migration.parent = None;
        self.migration.outcome = None;
        self.migration.selecting = true;
        self.migration.focus = 0;
        self.dialog = Some((self.main, Dialog::Migration));
        window::run(self.main, |parent| {
            rfd::AsyncFileDialog::new()
                .set_parent(parent)
                .set_title("选择迁移目标目录（保留原文件夹名称）")
        })
        .then(move |dialog| {
            let source = source.clone();
            Task::perform(
                async move { dialog.pick_folder().await.map(|f| f.path().to_path_buf()) },
                move |parent| Message::MigrationPicked(source.clone(), parent),
            )
        })
    }

    pub(super) fn migration_picked(
        &mut self,
        source: PathBuf,
        parent: Option<PathBuf>,
    ) -> Task<Message> {
        let Some(parent) = parent else {
            self.migration.selecting = false;
            self.dialog = Some((self.main, Dialog::Roots));
            return Task::none();
        };
        let data = self.data.clone();
        Task::perform(
            async move {
                let result = (|| -> anyhow::Result<_> {
                    let exe = std::env::current_exe()?;
                    let protected = vec![data, exe.parent().unwrap().to_path_buf()];
                    let (source, target) =
                        video_management::migration::destination(&source, &parent, &protected)?;
                    Ok((source, parent, target))
                })();
                result.map_err(|e| format!("{e:#}"))
            },
            Message::MigrationValidated,
        )
    }

    pub(super) fn migration_validated(
        &mut self,
        result: Result<(PathBuf, PathBuf, PathBuf), String>,
    ) {
        self.migration.selecting = false;
        match result {
            Ok((source, parent, destination)) => {
                self.migration.source = Some(source);
                self.migration.parent = Some(parent);
                self.migration.destination = Some(destination);
            }
            Err(message) => {
                self.migration.outcome = Some(Outcome {
                    message,
                    retry: false,
                })
            }
        }
        self.dialog = Some((self.main, Dialog::Migration));
    }

    pub(super) fn start_migration(&mut self, retry: bool) {
        if self.migration.busy()
            || self.updates.installing()
            || self.pending_delete.is_some()
            || self.deletion.busy()
            || self.service.is_none()
        {
            return;
        }
        let command = if retry {
            if !self.migration.retry() {
                return;
            }
            Command::RetryMigration
        } else {
            if self.migration.selecting {
                return;
            }
            let (Some(source), Some(parent), Some(_)) = (
                &self.migration.source,
                &self.migration.parent,
                &self.migration.destination,
            ) else {
                return;
            };
            Command::MigrateRoot {
                source: source.clone(),
                parent: parent.clone(),
            }
        };
        self.migration.outcome = None;
        self.migration.cancelled = false;
        self.migration.focus = 0;
        self.migration.progress = Some(Progress {
            stage: "正在释放播放和封面文件…".into(),
            ..Progress::preparing()
        });
        self.migration.pending = Some(command);
        self.migration.started = Some(Instant::now());
        self.dialog = Some((self.main, Dialog::Migration));
        self.stop_player();
        // Worker::shutdown waits for active decoders. Keep that wait off the UI thread.
        let mut covers = std::mem::take(&mut self.covers);
        match std::thread::Builder::new()
            .name("migration-release-covers".into())
            .spawn(move || {
                covers.stop();
                covers
            }) {
            Ok(worker) => self.migration.releasing = Some(worker),
            Err(error) => {
                self.migration.pending = None;
                self.migration.progress = None;
                self.migration.outcome = Some(Outcome {
                    message: format!("未开始迁移：{error}"),
                    retry,
                });
            }
        }
    }

    pub(super) fn cancel_migration(&mut self) {
        if !self
            .migration
            .progress
            .as_ref()
            .is_some_and(|p| p.cancellable)
        {
            return;
        }
        self.migration.cancelled = true;
        if let Some(progress) = &mut self.migration.progress {
            progress.stage = "正在安全取消，请稍候…".into();
            progress.cancellable = false;
        }
        if self.migration.pending.is_none() {
            self.send(Command::CancelMigration);
        }
    }

    pub(super) fn tick_migration(&mut self) -> Option<Task<Message>> {
        if self
            .migration
            .releasing
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
            && let Some(worker) = self.migration.releasing.take()
        {
            match worker.join() {
                Ok(covers) => self.covers = covers,
                Err(_) => {
                    self.migration.pending = None;
                    self.migration.progress = None;
                    self.migration.outcome = Some(Outcome {
                        message: "封面组件未能正常释放，未开始迁移。".into(),
                        retry: false,
                    });
                }
            }
        }
        if self.migration.pending.is_some() && self.migration.releasing.is_none() {
            let idle = self.playback_jobs.is_idle();
            let expired = self
                .migration
                .started
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(10));
            if self.migration.cancelled || (!idle && expired) {
                let retry = matches!(self.migration.pending, Some(Command::RetryMigration));
                self.migration.pending = None;
                self.migration.progress = None;
                self.migration.outcome = Some(Outcome {
                    message: if self.migration.cancelled {
                        "迁移已取消，文件保持原位。"
                    } else {
                        "播放器尚未释放文件，未开始迁移，请稍后重试。"
                    }
                    .into(),
                    retry,
                });
            } else if idle && let Some(command) = self.migration.pending.take() {
                self.send(command);
            }
        }
        if self.migration.closing && !self.migration.busy() {
            self.migration.closing = false;
            return Some(self.update(Message::Close(self.main)));
        }
        None
    }

    pub(super) fn migration_view(&self) -> Element<'_, Message> {
        let colors = self.colors();
        let state = &self.migration;
        let mut content = widget::column![heading("迁移监控文件夹", 22)].spacing(16);
        for (label, path) in [("原文件夹", &state.source), ("迁移至", &state.destination)] {
            if let Some(path) = path {
                content = content.push(
                    widget::column![
                        text(label).size(12).color(colors.muted),
                        text(path.to_string_lossy().to_string())
                            .size(13)
                            .wrapping(text::Wrapping::Glyph)
                    ]
                    .spacing(6),
                );
            }
        }
        let mut buttons = row![space::horizontal()].spacing(8);
        for (index, (label, message, primary_action)) in
            self.migration_actions().into_iter().enumerate()
        {
            buttons = buttons.push(
                button(button_text(label).size(13))
                    .padding([10, 16])
                    .style(move |theme, status| {
                        button_focus(
                            theme,
                            status,
                            state.focus == index,
                            if primary_action {
                                primary(theme, status)
                            } else {
                                secondary(theme, status)
                            },
                        )
                    })
                    .on_press(message),
            );
        }
        if let Some(progress) = &state.progress {
            content = content
                .push(row![motion::spinner(18.0), text(&progress.stage).size(14)].spacing(8));
            if let Some(path) = &progress.path {
                content = content.push(
                    text(path.to_string_lossy().to_string())
                        .size(12)
                        .wrapping(text::Wrapping::Glyph),
                );
            }
            if progress.total > 0 {
                content = content
                    .push(progress_bar(0.0..=progress.total as f32, progress.done as f32).girth(5))
                    .push(
                        text(format!(
                            "{} / {}",
                            bytes_label(progress.done),
                            bytes_label(progress.total)
                        ))
                        .size(12),
                    );
            }
            content = content.push(
                text("扫描、播放和其他操作暂时暂停。更新路径和清理期间请等待完成。")
                    .size(12)
                    .color(colors.muted),
            );
            if !progress.cancellable {
                buttons = buttons.push(
                    button(button_text("取消迁移").size(13))
                        .padding([10, 16])
                        .style(secondary),
                );
            }
        } else if state.selecting {
            content = content.push(
                row![
                    motion::spinner(18.0),
                    text("正在选择并检查目标目录…").size(13)
                ]
                .spacing(8),
            );
        } else if let Some(outcome) = &state.outcome {
            content = content.push(
                text(&outcome.message)
                    .size(13)
                    .wrapping(text::Wrapping::Glyph),
            );
        } else {
            content = content.push(text("整个文件夹及所有视频、字幕和其他文件将一起移动，保留目录结构。完成后继续监控新位置，保留标签与处理状态。").size(13).color(colors.muted));
        }
        widget::column![
            container(scrollable(content).height(Length::Shrink)).max_height(450),
            widget::rule::horizontal(1).style(separator),
            buttons
        ]
        .spacing(20)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::Named;

    fn app() -> (tempfile::TempDir, App) {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(temp.path().into(), None, None);
        if let Some(mut service) = app.service.take() {
            service.shutdown();
        }
        app.pending = Some(window::Id::unique());
        (temp, app)
    }

    #[test]
    fn confirmation_keyboard_can_cancel_without_moving_files() {
        let (_temp, mut app) = app();
        app.migration.source = Some(PathBuf::from(r"D:\视频"));
        app.migration.destination = Some(PathBuf::from(r"E:\归档\视频"));
        app.dialog = Some((app.main, Dialog::Migration));
        let _ = app.update(Message::NavigationKey(app.main, Named::Tab, false));
        assert_eq!(app.migration.focus, 1);
        let _ = app.update(Message::NavigationKey(app.main, Named::Tab, true));
        assert_eq!(app.migration.focus, 0);
        let _ = app.update(Message::NavigationKey(app.main, Named::Enter, false));
        assert!(matches!(app.dialog, Some((_, Dialog::Roots))));
        assert!(!app.migration.busy());
    }

    #[test]
    fn migration_blocks_both_windows_and_escape_requests_safe_cancel() {
        let (_temp, mut app) = app();
        app.migration.progress = Some(Progress::preparing());
        app.dialog = Some((app.main, Dialog::Migration));
        let _ = app.update(Message::Search(false, "changed".into()));
        let _ = app.update(Message::Search(true, "changed".into()));
        let _ = app.update(Message::ShowSettings);
        assert!(app.main_pane.search.is_empty() && app.pending_pane.search.is_empty());
        assert_eq!(app.page, MainPage::Library);
        let _ = app.update(Message::Stop);
        assert!(app.migration.cancelled);
        assert!(!app.migration.progress.as_ref().unwrap().cancellable);
        assert!(matches!(app.dialog, Some((_, Dialog::Migration))));
        assert!(app.migration_actions().is_empty());
    }

    #[test]
    fn close_waits_for_commit_and_release_worker_is_joined() {
        let (_temp, mut app) = app();
        app.migration.progress = Some(Progress {
            cancellable: false,
            ..Progress::preparing()
        });
        let _ = app.update(Message::Close(app.main));
        assert!(app.migration.closing);
        assert!(!app.migration.cancelled);
        assert!(app.migration.busy());
        app.migration.releasing = Some(std::thread::spawn(covers::Covers::default));
        while app
            .migration
            .releasing
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            std::thread::yield_now();
        }
        let _ = app.tick_migration();
        assert!(app.migration.releasing.is_none());
        app.migration.progress = None;
        assert!(app.tick_migration().is_some());
    }

    #[test]
    fn monitored_roots_keyboard_reaches_status_and_done_when_migration_needs_recovery() {
        let (_temp, mut app) = app();
        app.dialog = Some((app.main, Dialog::Roots));
        app.migration.outcome = Some(Outcome {
            message: "retry".into(),
            retry: true,
        });
        assert!(app.root_focused(&RootAction::Result));
        let _ = app.update(Message::NavigationKey(app.main, Named::Enter, false));
        assert!(matches!(app.dialog, Some((_, Dialog::Migration))));
        let _ = app.update(Message::Dismiss);
        let _ = app.update(Message::NavigationKey(app.main, Named::End, false));
        assert!(app.root_focused(&RootAction::Done));
        let _ = app.update(Message::NavigationKey(app.main, Named::Enter, false));
        assert!(app.dialog.is_none());
    }
}
