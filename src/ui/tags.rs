use super::*;
use iced::widget::column;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagAction {
    Add,
    Remove,
    Import,
}

impl TagAction {
    fn title(self) -> &'static str {
        match self {
            Self::Add => "添加标签",
            Self::Remove => "移除标签",
            Self::Import => "选择标签",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct TagPicker {
    pub action: TagAction,
    pub ids: Vec<i64>,
    pub selected: BTreeSet<i64>,
    focus: usize,
}

impl TagPicker {
    pub(super) fn check(&mut self, snapshot: &Snapshot, id: i64, on: bool) {
        if let Some(index) = self.choices(snapshot).iter().position(|tag| tag.id == id) {
            self.focus = index;
            if on {
                self.selected.insert(id);
            } else {
                self.selected.remove(&id);
            }
        }
    }
    fn choices<'a>(&self, snapshot: &'a Snapshot) -> Vec<&'a Tag> {
        snapshot
            .tags
            .iter()
            .filter(|tag| {
                self.action != TagAction::Remove
                    || snapshot
                        .videos
                        .iter()
                        .any(|v| self.ids.contains(&v.id) && v.tag_ids.contains(&tag.id))
            })
            .collect()
    }

    fn reconcile(&mut self, snapshot: &Snapshot) {
        let available: BTreeSet<_> = self.choices(snapshot).iter().map(|tag| tag.id).collect();
        self.selected.retain(|id| available.contains(id));
        self.focus = self.focus.min(available.len() + 1);
    }
}

impl App {
    pub(super) fn clear_tag_selection(&mut self, pending: bool) {
        let pane = self.pane_mut(pending);
        pane.selected.clear();
        pane.focus = None;
        pane.tag_draft.clear();
    }

    pub(super) fn reconcile_tags(&mut self) {
        let available: BTreeSet<_> = self.snapshot.tags.iter().map(|tag| tag.id).collect();
        self.main_pane.tag_draft.retain(|id| available.contains(id));
        self.pending_pane
            .tag_draft
            .retain(|id| available.contains(id));
        if let Some((_, Dialog::Tags(picker))) = &mut self.dialog {
            picker.reconcile(&self.snapshot);
        }
        if let Filter::Tag(id) = self.filter
            && !available.contains(&id)
        {
            self.filter = Filter::All;
            self.clear_tag_selection(false);
            self.main_pane.page = 0;
            self.main_pane.page_input = None;
        }
    }

    pub(super) fn open_tag_picker(&mut self, action: TagAction) {
        let pending = action == TagAction::Import;
        let ids = self.ids(pending);
        if ids.is_empty() {
            return;
        }
        let mut picker = TagPicker {
            action,
            ids,
            focus: 0,
            selected: if pending {
                self.pending_pane.tag_draft.clone()
            } else {
                BTreeSet::new()
            },
        };
        picker.reconcile(&self.snapshot);
        let owner = if pending {
            self.pending.unwrap_or(self.main)
        } else {
            self.main
        };
        self.dialog = Some((owner, Dialog::Tags(picker)));
    }

    pub(super) fn save_tag_selection(&mut self) {
        let Some((_, Dialog::Tags(picker))) = &mut self.dialog else {
            return;
        };
        picker.reconcile(&self.snapshot);
        if picker.action != TagAction::Import && picker.selected.is_empty() {
            return;
        }
        let Some((_, Dialog::Tags(picker))) = self.dialog.take() else {
            return;
        };
        if picker.action == TagAction::Import {
            self.pending_pane.tag_draft = picker.selected;
        } else {
            let tags = picker.selected.into_iter().collect();
            let assignment = match picker.action {
                TagAction::Add => TagAssignment::Add(tags),
                TagAction::Remove => TagAssignment::Remove(tags),
                TagAction::Import => unreachable!(),
            };
            self.send(Command::Assign(picker.ids, assignment, false));
            self.clear_tag_selection(false);
        }
    }

    pub(super) fn tag_picker_key(
        &mut self,
        key: iced::keyboard::key::Named,
        reverse: bool,
    ) -> Task<Message> {
        use iced::keyboard::key::Named;
        let Some((_, Dialog::Tags(picker))) = &mut self.dialog else {
            return Task::none();
        };
        let ids: Vec<_> = picker
            .choices(&self.snapshot)
            .iter()
            .map(|tag| tag.id)
            .collect();
        let count = ids.len() + 2;
        match key {
            Named::Tab | Named::ArrowDown | Named::ArrowUp => {
                let backwards = key == Named::ArrowUp || (key == Named::Tab && reverse);
                picker.focus = (picker.focus + if backwards { count - 1 } else { 1 }) % count;
            }
            Named::Home => picker.focus = 0,
            Named::End => picker.focus = ids.len().saturating_sub(1),
            Named::Space | Named::Enter => {
                if let Some(&id) = ids.get(picker.focus) {
                    if !picker.selected.remove(&id) {
                        picker.selected.insert(id);
                    }
                } else if picker.focus == ids.len() {
                    self.dialog = None;
                } else {
                    self.save_tag_selection();
                }
                return Task::none();
            }
            _ => return Task::none(),
        }
        if picker.focus < ids.len() {
            return iced::advanced::widget::operate(ScrollTagIntoView {
                target: format!("tag-option-{}", ids[picker.focus]).into(),
                viewport: None,
                option: None,
            });
        }
        Task::none()
    }

    pub(super) fn pending_tag_assignment(&self) -> TagAssignment {
        if self.pending_pane.tag_draft.is_empty() {
            TagAssignment::Keep
        } else {
            TagAssignment::Add(self.pending_pane.tag_draft.iter().copied().collect())
        }
    }

    pub(super) fn import_changes_shared_tags(
        &self,
        ids: &[i64],
        assignment: &TagAssignment,
    ) -> bool {
        match assignment {
            TagAssignment::Add(tags) => self.snapshot.videos.iter().any(|v| {
                v.library && ids.contains(&v.id) && tags.iter().any(|tag| !v.tag_ids.contains(tag))
            }),
            _ => false,
        }
    }

    pub(super) fn tag_names(&self, video: &VideoItem) -> Vec<&str> {
        self.snapshot
            .tags
            .iter()
            .filter(|tag| video.tag_ids.contains(&tag.id))
            .map(|tag| tag.name.as_str())
            .collect()
    }

    pub(super) fn tag_summary(&self, video: &VideoItem) -> String {
        let names = self.tag_names(video);
        if names.is_empty() {
            return "无标签".into();
        }
        let mut result = names
            .iter()
            .take(2)
            .map(|name| {
                let short: String = name.chars().take(16).collect();
                if name.chars().count() > 16 {
                    format!("{short}…")
                } else {
                    short
                }
            })
            .collect::<Vec<_>>()
            .join("、");
        if names.len() > 2 {
            result.push_str(&format!(" +{}", names.len() - 2));
        }
        result
    }

    pub(super) fn tag_count(&self, id: i64) -> usize {
        self.snapshot
            .videos
            .iter()
            .filter(|v| v.library && v.tag_ids.contains(&id))
            .count()
    }

    pub(super) fn tag_picker_view<'a>(&'a self, picker: &'a TagPicker) -> Element<'a, Message> {
        let colors = self.colors();
        let choices = picker.choices(&self.snapshot);
        let option_count = choices.len();
        let mut items = column![].spacing(8);
        for (index, tag) in choices.iter().enumerate() {
            let tag_id = tag.id;
            items = items.push(
                container(
                    row![
                        checkbox(picker.selected.contains(&tag_id))
                            .size(18)
                            .on_toggle(move |on| Message::TagCheck(tag_id, on)),
                        button(
                            text(&tag.name)
                                .size(14)
                                .line_height(iced::Pixels(20.0))
                                .wrapping(text::Wrapping::Glyph)
                        )
                        .style(quiet)
                        .padding(6)
                        .width(Fill)
                        .on_press(Message::TagCheck(
                            tag_id,
                            !picker.selected.contains(&tag_id)
                        )),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .id(format!("tag-option-{tag_id}"))
                .padding([4, 6])
                .style(move |theme| container::Style {
                    border: iced::Border {
                        color: if picker.focus == index {
                            palette(theme).accent
                        } else {
                            Color::TRANSPARENT
                        },
                        width: 2.0,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                }),
            );
        }
        if choices.is_empty() {
            items = items.push(
                text(if picker.action == TagAction::Remove {
                    "所选视频没有可移除的标签。"
                } else {
                    "还没有标签。可返回列表，点击“新建标签”。"
                })
                .size(13)
                .color(colors.muted)
                .wrapping(text::Wrapping::Glyph),
            );
        }
        column![
            heading(picker.action.title(), 22),
            text(match picker.action {
                TagAction::Add => "为所选视频添加勾选的标签，保留其他标签。",
                TagAction::Remove => "仅移除勾选的标签，保留其他标签。",
                TagAction::Import => "导入时添加勾选的标签；不勾选则保留已有标签。",
            })
            .size(13)
            .color(colors.muted),
            scrollable(items)
                .id("tag-options")
                .height(260)
                .direction(scrollable::Direction::Vertical(content_scrollbar())),
            text(format!(
                "已选 {} 个标签 · Tab / ↑↓ 切换，空格勾选",
                picker.selected.len()
            ))
            .size(12)
            .color(colors.muted),
            widget::rule::horizontal(1).style(separator),
            row![
                space::horizontal(),
                button(button_text("取消").size(13))
                    .width(100)
                    .padding([10, 16])
                    .style(move |theme, status| picker_button(
                        theme,
                        status,
                        false,
                        picker.focus == option_count
                    ))
                    .on_press(Message::Dismiss),
                button(
                    button_text(if picker.action == TagAction::Import {
                        "完成"
                    } else {
                        picker.action.title()
                    })
                    .size(13)
                )
                .width(100)
                .padding([10, 16])
                .style(move |theme, status| picker_button(
                    theme,
                    status,
                    true,
                    picker.focus == option_count + 1
                ))
                .on_press_maybe(
                    (picker.action == TagAction::Import || !picker.selected.is_empty())
                        .then_some(Message::SaveTagSelection)
                ),
            ]
            .spacing(8),
        ]
        .spacing(20)
        .into()
    }
}

// Measure the focused row after layout so wrapped names stay in view.
struct ScrollTagIntoView {
    target: iced::advanced::widget::Id,
    viewport: Option<(iced::Rectangle, iced::Rectangle)>,
    option: Option<iced::Rectangle>,
}

impl iced::advanced::widget::Operation<Message> for ScrollTagIntoView {
    fn traverse(
        &mut self,
        operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<Message>),
    ) {
        operate(self);
    }

    fn container(&mut self, id: Option<&iced::advanced::widget::Id>, bounds: iced::Rectangle) {
        if id == Some(&self.target) {
            self.option = Some(bounds);
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: iced::Rectangle,
        content_bounds: iced::Rectangle,
        _translation: iced::Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if id == Some(&iced::advanced::widget::Id::from("tag-options")) {
            self.viewport = Some((bounds, content_bounds));
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<Message> {
        use iced::advanced::widget::operation::{Outcome, scrollable};
        match (self.viewport, self.option) {
            (Some((viewport, content)), Some(option)) => {
                Outcome::Chain(Box::new(scrollable::scroll_to(
                    "tag-options".into(),
                    scrollable::AbsoluteOffset {
                        x: None,
                        y: Some((option.y + option.height - content.y - viewport.height).max(0.0)),
                    },
                )))
            }
            _ => Outcome::None,
        }
    }
}

fn picker_button(
    theme: &iced::Theme,
    status: widget::button::Status,
    prominent: bool,
    focused: bool,
) -> widget::button::Style {
    let mut style = if prominent {
        primary(theme, status)
    } else {
        secondary(theme, status)
    };
    if focused && status != widget::button::Status::Disabled {
        style.border.color = if prominent {
            palette(theme).ink
        } else {
            palette(theme).accent
        };
        style.border.width = 2.0;
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: i64, tags: &[i64], library: bool, pending: bool) -> VideoItem {
        VideoItem {
            id,
            hash: format!("{id:064x}"),
            title: format!("视频{id}"),
            tag_ids: tags.to_vec(),
            library,
            created_at: id,
            files: vec![VideoFile {
                id,
                video_id: id,
                path: format!("C:\\视频\\{id}.mp4").into(),
                size: 10,
                modified: 100,
                availability: Availability::Present,
                state: if pending {
                    FileState::Pending
                } else {
                    FileState::Managed
                },
            }],
        }
    }

    fn app() -> (tempfile::TempDir, App) {
        let temp = tempfile::tempdir().unwrap();
        // No window task is executed. Stop the temporary service before supplying a fixture.
        let (mut app, _) = App::boot(temp.path().to_path_buf(), None, None);
        if let Some(mut service) = app.service.take() {
            service.shutdown();
        }
        app.library_loading = false;
        app.pending = Some(window::Id::unique());
        app.snapshot = Snapshot {
            tags: (1..=4)
                .map(|id| Tag {
                    id,
                    name: format!("标签{id}"),
                })
                .collect(),
            videos: vec![
                video(1, &[1, 2], true, true),
                video(2, &[2, 3], true, false),
                video(3, &[], true, false),
                video(4, &[1], false, true),
            ],
            roots: vec![],
        };
        (temp, app)
    }

    fn picker(app: &App) -> &TagPicker {
        let Some((_, Dialog::Tags(picker))) = &app.dialog else {
            panic!("missing tag picker")
        };
        picker
    }

    #[test]
    fn single_tag_filters_counts_and_summaries_support_multiple_tags() {
        let (_temp, mut app) = app();
        let mut copy = app.snapshot.videos[0].files[0].clone();
        copy.id = 10;
        app.snapshot.videos[0].files.push(copy);
        assert_eq!(app.tag_count(1), 1);
        assert_eq!(app.tag_count(2), 2);
        let _ = app.update(Message::Filter(Filter::Tag(2)));
        assert_eq!(
            app.filtered(false)
                .iter()
                .map(|v| v.id)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([1, 2])
        );
        let _ = app.update(Message::Filter(Filter::Untagged));
        assert_eq!(
            app.filtered(false).iter().map(|v| v.id).collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(app.filtered(true).len(), 2);
        app.snapshot.videos[0].tag_ids.push(3);
        assert_eq!(app.tag_summary(&app.snapshot.videos[0]), "标签1、标签2 +1");
        assert_eq!(
            app.tag_names(&app.snapshot.videos[0]),
            vec!["标签1", "标签2", "标签3"]
        );
        assert_eq!(app.tag_summary(&app.snapshot.videos[2]), "无标签");
    }

    #[test]
    fn batch_picker_starts_empty_cancel_discards_and_remove_uses_union() {
        let (_temp, mut app) = app();
        app.main_pane.selected.extend([1, 2]);
        let _ = app.update(Message::PickTags(TagAction::Add));
        assert!(picker(&app).selected.is_empty());
        assert_eq!(picker(&app).choices(&app.snapshot).len(), 4);
        let _ = app.update(Message::SaveTagSelection);
        assert!(app.dialog.is_some());
        let _ = app.update(Message::TagCheck(1, true));
        let _ = app.update(Message::TagCheck(2, true));
        assert_eq!(picker(&app).selected, BTreeSet::from([1, 2]));
        let _ = app.update(Message::Dismiss);
        let _ = app.update(Message::PickTags(TagAction::Remove));
        assert!(picker(&app).selected.is_empty());
        assert_eq!(
            picker(&app)
                .choices(&app.snapshot)
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        let _ = app.update(Message::TagCheck(4, true));
        assert!(picker(&app).selected.is_empty());
        let _ = app.update(Message::TagCheck(2, true));
        let _ = app.update(Message::SaveTagSelection);
        assert!(app.dialog.is_none());
        assert!(app.main_pane.selected.is_empty());
        assert!(app.main_pane.focus.is_none());
    }

    #[test]
    fn pending_drafts_preserve_tags_and_confirm_only_changes_to_shared_content() {
        let (_temp, mut app) = app();
        app.pending_pane.selected.insert(1);
        assert_eq!(app.pending_tag_assignment(), TagAssignment::Keep);
        let _ = app.update(Message::PickTags(TagAction::Import));
        let _ = app.update(Message::TagCheck(1, true));
        let _ = app.update(Message::SaveTagSelection);
        assert_eq!(app.pending_tag_assignment(), TagAssignment::Add(vec![1]));
        assert!(!app.import_changes_shared_tags(&[1], &app.pending_tag_assignment()));
        let _ = app.update(Message::PickTags(TagAction::Import));
        let _ = app.update(Message::TagCheck(3, true));
        let _ = app.update(Message::Dismiss);
        assert_eq!(app.pending_pane.tag_draft, BTreeSet::from([1]));
        let _ = app.update(Message::PickTags(TagAction::Import));
        let _ = app.update(Message::TagCheck(3, true));
        let _ = app.update(Message::SaveTagSelection);
        let _ = app.update(Message::Assign(true));
        assert!(
            matches!(&app.dialog, Some((_, Dialog::Assign { ids, assignment: TagAssignment::Add(tags), accept: true })) if ids == &[1] && tags == &[1,3])
        );
        let _ = app.update(Message::ConfirmAssign);
        assert!(app.pending_pane.tag_draft.is_empty());
        assert!(app.pending_pane.selected.is_empty());
        app.pending_pane.selected.insert(4);
        let _ = app.update(Message::Assign(true));
        assert!(app.dialog.is_none());
        assert!(app.pending_pane.selected.is_empty());
    }

    #[test]
    fn cross_page_selection_is_kept_and_search_filter_clear_tag_drafts() {
        let (_temp, mut app) = app();
        app.snapshot.videos = (1..=65).map(|id| video(id, &[1], true, true)).collect();
        let _ = app.update(Message::PageSize(false, PageSize::Twenty));
        let _ = app.update(Message::Check(false, 65, true));
        let _ = app.update(Message::Page(false, 3));
        let _ = app.update(Message::Check(false, 1, true));
        let _ = app.update(Message::Sort(false, Sort::Name));
        let _ = app.update(Message::PickTags(TagAction::Add));
        assert_eq!(picker(&app).ids, vec![1, 65]);
        let _ = app.update(Message::Dismiss);
        app.main_pane.tag_draft.insert(1);
        let _ = app.update(Message::Filter(Filter::Tag(1)));
        assert!(app.main_pane.selected.is_empty());
        assert!(app.main_pane.tag_draft.is_empty());
        app.pending_pane.selected.insert(1);
        app.pending_pane.tag_draft.insert(2);
        let _ = app.update(Message::Page(true, 1));
        assert_eq!(app.pending_pane.tag_draft, BTreeSet::from([2]));
        let _ = app.update(Message::Search(true, "视频".into()));
        assert!(app.pending_pane.selected.is_empty());
        assert!(app.pending_pane.tag_draft.is_empty());
        app.pending_pane.tag_draft.insert(1);
        let _ = app.update(Message::ClearSelection(true));
        assert!(app.pending_pane.tag_draft.is_empty());
    }

    #[test]
    fn deleted_tags_are_removed_from_drafts_and_active_filter_returns_to_all() {
        let (_temp, mut app) = app();
        app.filter = Filter::Tag(2);
        app.main_pane.selected.insert(1);
        app.pending_pane.tag_draft.extend([1, 2]);
        let _ = app.update(Message::PickTags(TagAction::Remove));
        let _ = app.update(Message::TagCheck(2, true));
        app.snapshot.tags.retain(|t| t.id != 2);
        app.reconcile_tags();
        assert_eq!(app.filter, Filter::All);
        assert_eq!(app.pending_pane.tag_draft, BTreeSet::from([1]));
        assert!(picker(&app).selected.is_empty());
        let _ = app.update(Message::SaveTagSelection);
        assert!(app.dialog.is_some());
    }

    #[test]
    fn tag_picker_keyboard_focus_toggle_cancel_and_apply() {
        use iced::keyboard::key::Named;
        let (_temp, mut app) = app();
        app.pending_pane.selected.insert(4);
        let _ = app.update(Message::PickTags(TagAction::Import));
        let pending = app.pending.unwrap();
        let _ = app.update(Message::NavigationKey(pending, Named::Space, false));
        let _ = app.update(Message::NavigationKey(pending, Named::ArrowDown, false));
        let _ = app.update(Message::NavigationKey(pending, Named::Space, false));
        assert_eq!(picker(&app).selected, BTreeSet::from([1, 2]));
        let _ = app.update(Message::NavigationKey(pending, Named::Home, false));
        let _ = app.update(Message::NavigationKey(pending, Named::Tab, true));
        let _ = app.update(Message::NavigationKey(pending, Named::Enter, false));
        assert!(app.dialog.is_none());
        assert_eq!(app.pending_pane.tag_draft, BTreeSet::from([1, 2]));
        let _ = app.update(Message::PickTags(TagAction::Import));
        let _ = app.update(Message::TagCheck(3, true));
        let _ = app.update(Message::Stop);
        assert!(app.dialog.is_none());
        assert_eq!(app.pending_pane.tag_draft, BTreeSet::from([1, 2]));
    }
}
