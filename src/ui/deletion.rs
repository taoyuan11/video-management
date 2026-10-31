//! Persistent, non-modal feedback for confirmed file deletion in both windows.
use super::*;
use iced::widget::column;

#[derive(Default)]
pub(super) struct DeletionUi {
    targets: Vec<DeleteTarget>,
    pub progress: Option<DeleteProgress>,
    pub outcome: Option<DeleteOutcome>,
    pub expanded: bool,
}

impl DeletionUi {
    pub fn busy(&self) -> bool {
        self.progress.is_some()
    }

    pub fn contains(&self, id: i64) -> bool {
        self.busy() && self.targets.iter().any(|target| target.file_id == id)
    }

    pub fn start(&mut self, targets: Vec<DeleteTarget>) {
        *self = Self {
            progress: Some(DeleteProgress {
                stage: DeleteStage::Queued,
                completed: 0,
                total: targets.len(),
                path: None,
            }),
            targets,
            ..Default::default()
        };
    }

    pub fn finish(&mut self, outcome: DeleteOutcome) {
        self.progress = None;
        self.expanded = !outcome.failures.is_empty();
        self.outcome = Some(outcome);
    }

    fn title(&self) -> &'static str {
        if self.busy() {
            "删除中"
        } else if let Some(outcome) = &self.outcome {
            if outcome.failures.is_empty() {
                "删除成功"
            } else if outcome.deleted == 0 {
                "删除失败"
            } else {
                "部分删除失败"
            }
        } else {
            ""
        }
    }

    fn summary(&self) -> String {
        if let Some(progress) = &self.progress {
            let stage = match progress.stage {
                DeleteStage::WaitingForPlayer => "正在等待播放器释放文件…",
                DeleteStage::Queued => "已提交，正在等待后台处理…",
                DeleteStage::Preparing => "正在检查删除请求…",
                DeleteStage::WaitingForScan => "正在等待扫描任务结束…",
                DeleteStage::Verifying => "正在校验文件内容，大文件可能需要较长时间…",
                DeleteStage::Removing => "正在永久删除文件并更新资料库…",
            };
            format!(
                "{stage}  已处理 {} / {} 个文件",
                progress.completed, progress.total
            )
        } else if let Some(outcome) = &self.outcome {
            if outcome.failures.is_empty() {
                format!("已永久删除 {} 个文件，未选中的副本保留。", outcome.deleted)
            } else {
                format!(
                    "已删除 {} / {} 个文件 · {} 项异常，请查看原因。",
                    outcome.deleted,
                    outcome.total,
                    outcome.failures.len()
                )
            }
        } else {
            String::new()
        }
    }

    fn report(&self) -> String {
        let mut report = format!("{}\n{}", self.title(), self.summary());
        if let Some(outcome) = &self.outcome {
            for failure in &outcome.failures {
                report.push_str(&format!(
                    "\n\n{}\n{}",
                    failure.path.display(),
                    failure.reason
                ));
            }
        }
        report
    }
}

impl App {
    pub(super) fn dispatch_delete(&mut self, video_id: i64, targets: Vec<DeleteTarget>) {
        if self.service.as_ref().is_some_and(|service| {
            service
                .commands
                .send(Command::Delete(video_id, targets))
                .is_ok()
        }) {
            if let Some(progress) = &mut self.deletion.progress {
                progress.stage = DeleteStage::Queued;
            }
        } else {
            self.fail_delete("后台服务不可用，未删除。请重新打开软件后重试。");
        }
    }

    pub(super) fn fail_delete(&mut self, reason: &str) {
        if !self.deletion.busy() {
            return;
        }
        self.deletion.finish(DeleteOutcome {
            total: self.deletion.targets.len(),
            deleted: 0,
            failures: self
                .deletion
                .targets
                .iter()
                .map(|target| DeleteFailure {
                    path: target.path.clone(),
                    reason: reason.into(),
                })
                .collect(),
        });
    }

    pub(super) fn delete_notice(&self) -> Option<Element<'_, Message>> {
        let state = &self.deletion;
        if !state.busy() && state.outcome.is_none() {
            return None;
        }
        let colors = self.colors();
        let (tone, color, glyph) = if state.busy() {
            (FeedbackTone::Information, colors.accent, INFO)
        } else if let Some(outcome) = &state.outcome {
            if outcome.failures.is_empty() {
                (FeedbackTone::Success, colors.success, CHECK)
            } else if outcome.deleted > 0 {
                (FeedbackTone::Warning, colors.warning, WARNING)
            } else {
                (FeedbackTone::Error, colors.danger, WARNING)
            }
        } else {
            return None;
        };
        let status: Element<_> = if state.busy() {
            motion::spinner(16.0)
        } else {
            icon(glyph, 16).color(color).into()
        };
        let mut header = row![status, heading(state.title(), 14).color(color).width(Fill),]
            .spacing(8)
            .align_y(Alignment::Center);
        if let Some(outcome) = &state.outcome {
            if !outcome.failures.is_empty() {
                header = header.push(
                    button(
                        button_text(if state.expanded {
                            "收起原因"
                        } else {
                            "查看原因"
                        })
                        .size(12),
                    )
                    .padding([6, 8])
                    .style(quiet)
                    .on_press(Message::ToggleDeleteDetails),
                );
            }
            header = header
                .push(
                    button(label(COPY, "复制结果"))
                        .padding([6, 8])
                        .style(quiet)
                        .on_press(Message::Copy(state.report())),
                )
                .push(
                    button(label(CLOSE, "关闭"))
                        .padding([6, 8])
                        .style(quiet)
                        .on_press(Message::ClearDeleteNotice),
                );
        }
        let mut content = column![
            header,
            text(state.summary())
                .size(12)
                .color(colors.ink)
                .wrapping(text::Wrapping::Glyph),
        ]
        .spacing(8);
        if let Some(progress) = &state.progress {
            if let Some(path) = &progress.path {
                content = content.push(
                    container(scrollable(
                        text(path.to_string_lossy().to_string())
                            .size(12)
                            .color(colors.muted)
                            .width(Fill)
                            .wrapping(text::Wrapping::Glyph),
                    ))
                    .max_height(48),
                );
            }
            if progress.total > 1 {
                content = content.push(
                    progress_bar(0.0..=progress.total as f32, progress.completed as f32).girth(3),
                );
            }
        }
        if state.expanded
            && let Some(outcome) = &state.outcome
        {
            let mut failures = column![].spacing(12);
            for failure in &outcome.failures {
                failures = failures.push(
                    column![
                        text(failure.path.to_string_lossy().to_string())
                            .size(12)
                            .color(colors.ink)
                            .width(Fill)
                            .wrapping(text::Wrapping::Glyph),
                        text(&failure.reason)
                            .size(12)
                            .color(colors.muted)
                            .width(Fill)
                            .wrapping(text::Wrapping::Glyph),
                    ]
                    .spacing(4),
                );
            }
            content = content.push(container(scrollable(failures).spacing(12)).max_height(128));
        }
        Some(
            container(content)
                .padding(12)
                .width(Fill)
                .style(move |theme| feedback(theme, tone))
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets() -> Vec<DeleteTarget> {
        (1..=2)
            .map(|id| DeleteTarget {
                file_id: id,
                path: format!(
                    "C:\\删除测试\\{}\\视频 {id}.mp4",
                    "很长的文件夹名称".repeat(25)
                )
                .into(),
                hash: format!("{id:064x}"),
            })
            .collect()
    }

    #[test]
    fn confirming_in_either_window_immediately_shows_progress_and_prevents_resubmission() {
        for pending in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let (mut app, _) = App::boot(temp.path().into(), None, None);
            app.pending = Some(window::Id::unique());
            app.dialog = Some((
                if pending {
                    app.pending.unwrap()
                } else {
                    app.main
                },
                Dialog::Delete {
                    video_id: 1,
                    files: vec![],
                    selected: BTreeSet::from([1]),
                    targets: targets(),
                },
            ));
            let _ = app.update(Message::ConfirmDelete);
            assert!(app.dialog.is_none());
            assert!(app.deletion.busy());
            assert_eq!(app.deletion.title(), "删除中");
            assert_eq!(app.deletion.progress.as_ref().unwrap().total, 1);
            assert!(app.deletion.contains(1));
            assert!(!app.deletion.contains(2));
            assert!(app.delete_notice().is_some());
            let _ = app.update(Message::ClearDeleteNotice);
            let _ = app.update(Message::ConfirmDelete);
            assert!(app.deletion.busy());
            let _ = app.update(Message::Play(1, app.main));
            assert!(app.loader.is_none());
            assert!(app.error.as_deref().unwrap().contains("正在删除"));
            // App::drop joins the service; no window task is executed.
        }
    }

    #[test]
    fn deletion_results_survive_scan_events_and_keep_every_failure_until_dismissed() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(temp.path().into(), None, None);
        let service = app.service.as_mut().unwrap();
        service.shutdown();
        let (tx, rx) = crossbeam_channel::unbounded();
        service.events = rx;
        let targets = targets();
        app.deletion.start(targets.clone());
        tx.send(Event::DeleteProgress(DeleteProgress {
            stage: DeleteStage::Verifying,
            completed: 1,
            total: 2,
            path: Some(targets[1].path.clone()),
        }))
        .unwrap();
        tx.send(Event::Status("扫描完成".into())).unwrap();
        let _ = app.update(Message::Tick);
        assert!(app.deletion.busy());
        assert!(app.deletion.summary().contains("校验"));
        tx.send(Event::DeleteFinished(DeleteOutcome {
            total: 2,
            deleted: 1,
            failures: vec![DeleteFailure {
                path: targets[1].path.clone(),
                reason: "文件被占用，请关闭占用程序后重试。".into(),
            }],
        }))
        .unwrap();
        tx.send(Event::Status("正在扫描…".into())).unwrap();
        let _ = app.update(Message::Tick);
        assert!(!app.deletion.busy());
        assert_eq!(app.deletion.title(), "部分删除失败");
        assert!(app.deletion.expanded);
        assert!(
            app.deletion
                .report()
                .contains(&targets[1].path.to_string_lossy().to_string())
        );
        assert!(app.deletion.report().contains("占用"));
        assert!(app.deletion.summary().contains("1 / 2"));
        let _ = app.update(Message::ToggleDeleteDetails);
        assert!(!app.deletion.expanded);
        let _ = app.update(Message::Tick);
        assert!(app.delete_notice().is_some());
        let _ = app.update(Message::ClearDeleteNotice);
        assert!(app.delete_notice().is_none());
    }

    #[test]
    fn service_failure_finishes_progress_and_success_is_never_replaced_by_scan_status() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(temp.path().into(), None, None);
        let service = app.service.as_mut().unwrap();
        service.shutdown();
        let (tx, rx) = crossbeam_channel::unbounded();
        service.events = rx;
        app.deletion.start(targets());
        tx.send(Event::Stopped).unwrap();
        let _ = app.update(Message::Tick);
        assert_eq!(app.deletion.title(), "删除失败");
        assert!(!app.deletion.busy());
        assert!(app.deletion.report().contains("无法确认删除结果"));
        app.deletion.start(targets());
        app.dispatch_delete(1, targets());
        assert_eq!(app.deletion.title(), "删除失败");
        assert!(app.deletion.report().contains("未删除"));
        app.deletion.finish(DeleteOutcome {
            total: 2,
            deleted: 2,
            failures: vec![],
        });
        let _ = app.update(Message::Tick);
        assert_eq!(app.deletion.title(), "删除成功");
        assert!(!app.deletion.expanded);
        assert!(app.delete_notice().is_some());
        let _ = app.update(Message::ClearDeleteNotice);
        assert!(app.delete_notice().is_none());
    }

    #[test]
    fn long_paths_and_many_errors_leave_a_bounded_notice_at_minimum_workspace_widths() {
        use iced::{
            Renderer, Size,
            advanced::{layout, renderer::Headless, widget::Tree},
            futures::FutureExt,
        };
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(temp.path().into(), None, None);
        if let Some(mut service) = app.service.take() {
            service.shutdown();
        }
        let renderer = Renderer::new(iced::Font::DEFAULT, iced::Pixels(14.0), Some("tiny-skia"))
            .now_or_never()
            .flatten()
            .expect("headless software renderer");
        for theme in [ThemePreference::Light, ThemePreference::Dark] {
            app.app_settings.theme = theme;
            for failed in [false, true] {
                app.deletion.start(targets());
                if failed {
                    app.deletion.finish(DeleteOutcome {
                        total: 30,
                        deleted: 0,
                        failures: (0..30)
                            .map(|_| DeleteFailure {
                                path: targets()[0].path.clone(),
                                reason: "文件正被占用。请关闭占用程序后重试。".into(),
                            })
                            .collect(),
                    });
                } else {
                    app.deletion.progress.as_mut().unwrap().path = Some(targets()[0].path.clone());
                }
                // Both supported windows provide at least this much workspace.
                for width in [480.0, 700.0, 912.0] {
                    let mut notice = app.delete_notice().unwrap();
                    let mut tree = Tree::new(&notice);
                    let node = notice.as_widget_mut().layout(
                        &mut tree,
                        &renderer,
                        &layout::Limits::new(Size::ZERO, Size::new(width, 400.0)),
                    );
                    assert!(node.size().width <= width);
                    assert!(
                        node.size().height <= 260.0,
                        "feedback must leave space for content at width {width}: {:?}",
                        node.size()
                    );
                }
            }
        }
    }
}
