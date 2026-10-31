use super::*;
use iced::widget::column;
use std::path::Path;

fn modified_cell(video: &VideoItem, pending: bool, colors: Palette) -> Element<'_, Message> {
    let value: Element<_> =
        if let Some((date, time)) = listing::modified_label(listing::modified(video, pending)) {
            row![text(date).size(11), text(time).size(11)]
                .spacing(4)
                .into()
        } else {
            text("未知").size(12).into()
        };
    container(value)
        .width(LIST_MODIFIED_WIDTH)
        .style(move |_| container::Style {
            text_color: Some(colors.muted),
            ..Default::default()
        })
        .into()
}

fn page_arrow(glyph: char, message: Option<Message>) -> Element<'static, Message> {
    button(button_icon(glyph, 12))
        .style(quiet)
        .padding([6, 8])
        .on_press_maybe(message)
        .into()
}

impl App {
    fn section_title(&self, pending: bool) -> String {
        if pending {
            return "待处理视频".into();
        }
        match self.filter {
            Filter::All => "全部视频".into(),
            Filter::Untagged => "无标签".into(),
            Filter::Duplicates => "重复副本".into(),
            Filter::Missing => "缺失文件".into(),
        }
    }

    pub fn view(&self, id: window::Id) -> Element<'_, Message> {
        let colors = self.colors();
        if self.fullscreen && id == self.player_window {
            return container(self.player_view(id, true))
                .padding(24)
                .width(Fill)
                .height(Fill)
                .style(move |_| container::Style {
                    background: Some(colors.surface.into()),
                    ..Default::default()
                })
                .into();
        }
        let pending = Some(id) == self.pending;
        let showing_settings = self.page == MainPage::Settings && !pending;
        let showing_tags = self.page == MainPage::Tags && !pending;
        let header = widget::responsive(move |size| {
            self.workspace_header(pending, !pending && size.width < HEADER_COMPACT_WIDTH)
        })
        .height(Length::Shrink);
        let body = widget::responsive(move |size| {
            let maximum = resize::maximum_width(size.width);
            let width = self.details_width[usize::from(pending)].clamp(DETAILS_MIN_WIDTH, maximum);
            let visible = self.details_visible[usize::from(pending)];
            let enabled = visible && self.dialog.as_ref().is_none_or(|(owner, _)| *owner != id);
            row![
                widget::responsive(move |size| {
                    self.library(pending, size.width < LIST_COMPACT_WIDTH)
                }),
                motion::details(
                    visible,
                    width,
                    row![
                        resize::handle(pending, width, maximum, enabled),
                        self.details(pending, id)
                    ]
                    .height(Fill)
                )
            ]
            .height(Fill)
            .into()
        });
        let mut workspace = column![header].spacing(24);
        if pending {
            workspace = workspace.push(
                container(
                    row![
                        icon(INFO, 16).color(colors.accent),
                        text("导入前可预览或忽略。相同内容会自动合并，原文件保持原位置。")
                            .size(13)
                            .width(Fill),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(12)
                .width(Fill)
                .style(move |_| container::Style {
                    background: Some(colors.tint.into()),
                    border: iced::border::rounded(8),
                    ..Default::default()
                }),
            );
        }
        if let Some(notice) = &self.update_notice {
            workspace = workspace.push(
                container(
                    row![
                        icon(INFO, 16).color(colors.accent),
                        text(notice)
                            .size(13)
                            .width(Fill)
                            .wrapping(text::Wrapping::Glyph),
                        button(label(CLOSE, "关闭"))
                            .style(quiet)
                            .on_press(Message::ClearUpdateNotice),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .padding(12)
                .width(Fill)
                .style(inset),
            );
        }
        if let Some(error) = &self.error {
            workspace = workspace.push(
                container(
                    row![
                        icon(WARNING, 16).color(colors.danger),
                        text(error)
                            .size(13)
                            .width(Fill)
                            .wrapping(text::Wrapping::Glyph),
                        button(label(CLOSE, "关闭"))
                            .style(quiet)
                            .on_press(Message::ClearError),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .padding(12)
                .width(Fill)
                .style(move |_| container::Style {
                    background: Some(colors.error_surface.into()),
                    border: iced::border::rounded(8),
                    ..Default::default()
                }),
            );
        }
        let body: Element<_> = if showing_settings {
            self.settings_view()
        } else if showing_tags {
            self.tag_management_view()
        } else {
            body.into()
        };
        workspace = workspace.push(body);
        let workspace = container(workspace).padding(24).width(Fill).height(Fill);
        let main: Element<_> = if pending {
            workspace.into()
        } else {
            row![self.sidebar(), workspace].height(Fill).into()
        };
        let material = self.material_windows.contains(&id);
        // Leave the native backdrop visible through the shell and low-opacity cards.
        // Never make the HWND uniformly translucent: foreground content stays crisp.
        let base: Element<_> = container(column![main, self.status_bar()].height(Fill))
            .width(Fill)
            .height(Fill)
            .style(move |_| container::Style {
                background: Some(
                    if material {
                        colors.shell
                    } else {
                        colors.backdrop
                    }
                    .into(),
                ),
                ..Default::default()
            })
            .into();
        if let Some((owner, dialog)) = &self.dialog
            && *owner == id
        {
            let modal = container(self.dialog_view(dialog))
                .padding(28)
                .width(580)
                .max_height(620)
                .style(move |theme| container::Style {
                    background: Some(colors.modal.into()),
                    shadow: iced::Shadow {
                        color: Color::from_rgba(0.0, 0.0, 0.0, 0.18),
                        offset: iced::Vector::new(0.0, 8.0),
                        blur_radius: 32.0,
                    },
                    ..panel(theme)
                });
            let overlay = container(motion::popup(std::mem::discriminant(dialog), modal))
                .center_x(Fill)
                .center_y(Fill)
                .style(move |_| container::Style {
                    background: Some(colors.scrim.into()),
                    ..Default::default()
                });
            return stack![base, widget::opaque(overlay)].into();
        }
        base
    }

    fn workspace_header(&self, pending: bool, compact: bool) -> Element<'_, Message> {
        let colors = self.colors();
        if !pending && self.page == MainPage::Tags {
            return self.tag_management_header(compact);
        }
        if !pending && self.page == MainPage::Settings {
            let title = column![
                heading("设置", 28),
                text("按你的习惯调整视频处理与外观")
                    .size(13)
                    .color(colors.muted)
            ]
            .spacing(8)
            .width(Fill);
            return self.page_header(title.into(), None, true, compact);
        }
        let count = self.filtered(pending).len();
        let description = if pending {
            "预览新发现的视频，选择标签后加入资料库"
        } else {
            match self.filter {
                Filter::All => "集中浏览与整理你的本地视频",
                Filter::Untagged => "为这些视频找到合适的位置",
                Filter::Duplicates => "相同内容已合并，在详情中查看每个副本",
                Filter::Missing => "查看离线、已移动或暂时无法访问的文件",
            }
        };
        let title = column![
            row![
                heading(self.section_title(pending), 28),
                badge(colors, count.to_string(), false)
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            text(description).size(13).color(colors.muted),
        ]
        .spacing(8)
        .width(Fill);
        let actions = row![
            button(label(FOLDER, "添加文件夹"))
                .style(secondary)
                .padding([10, 14])
                .on_press(Message::PickRoots),
            button(label(ADD, "导入视频"))
                .style(primary)
                .padding([10, 16])
                .on_press(Message::Import),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        self.page_header(title.into(), Some(actions.into()), !pending, compact)
    }

    pub(super) fn page_header<'a>(
        &'a self,
        title: Element<'a, Message>,
        actions: Option<Element<'a, Message>>,
        show_pending: bool,
        compact: bool,
    ) -> Element<'a, Message> {
        let mut header = row![title].spacing(16).align_y(Alignment::Center);
        // Keep the window launcher at the top right, even when page commands wrap.
        if compact {
            if show_pending {
                header = header.push(self.pending_window_button());
            }
            let mut content = column![header].spacing(12);
            if let Some(actions) = actions {
                content = content.push(container(actions).align_right(Fill));
            }
            content.into()
        } else {
            if let Some(actions) = actions {
                header = header.push(actions);
            }
            if show_pending {
                header = header.push(self.pending_window_button());
            }
            header.into()
        }
    }

    fn pending_window_button(&self) -> Element<'_, Message> {
        let pending = self.snapshot.videos.iter().filter(|v| v.pending()).count();
        button(
            row![
                label(OPEN_WINDOW, "待处理视频"),
                badge(self.colors(), pending.to_string(), pending > 0)
            ]
            .spacing(8)
            .height(BUTTON_CONTENT_HEIGHT)
            .align_y(Alignment::Center),
        )
        .style(secondary)
        .padding([10, 14])
        .on_press(Message::OpenPending)
        .into()
    }

    fn nav_item(
        &self,
        glyph: char,
        title: String,
        count: usize,
        filter: Filter,
    ) -> Element<'_, Message> {
        self.navigation_item(
            glyph,
            title,
            count,
            self.page == MainPage::Library && self.filter == filter,
            Message::Filter(filter),
        )
    }

    fn navigation_item(
        &self,
        glyph: char,
        title: String,
        count: usize,
        active: bool,
        message: Message,
    ) -> Element<'_, Message> {
        let colors = self.colors();
        let marker =
            container(space::horizontal().width(3).height(18)).style(move |_| container::Style {
                background: Some(
                    if active {
                        colors.accent
                    } else {
                        Color::TRANSPARENT
                    }
                    .into(),
                ),
                border: iced::border::rounded(2),
                ..Default::default()
            });
        button(
            row![
                marker,
                icon(glyph, 17),
                text(title)
                    .size(13)
                    .wrapping(text::Wrapping::Glyph)
                    .width(Fill),
                text(count.to_string()).size(12).color(colors.muted)
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .padding([11, 8])
        .width(Fill)
        .on_press(message)
        .style(move |theme, status| item(theme, active, status))
        .into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let colors = self.colors();
        let library: Vec<_> = self.snapshot.videos.iter().filter(|v| v.library).collect();
        let brand = row![
            widget::image(branding::logo()).width(40).height(40),
            column![
                heading("帧库", 20),
                text("本地视频资料库").size(11).color(colors.muted)
            ]
            .spacing(2),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        let navigation = column![
            container(text("资料库").size(12).color(colors.muted)).padding([0, 12]),
            self.nav_item(LIBRARY, "全部视频".into(), library.len(), Filter::All),
            self.nav_item(
                TAG,
                "无标签".into(),
                library.iter().filter(|v| v.tag_ids.is_empty()).count(),
                Filter::Untagged
            ),
            self.nav_item(
                COPY,
                "重复副本".into(),
                library
                    .iter()
                    .filter(|v| v
                        .managed_files()
                        .filter(|f| f.availability == Availability::Present)
                        .count()
                        > 1)
                    .count(),
                Filter::Duplicates
            ),
            self.nav_item(
                WARNING,
                "缺失文件".into(),
                library.iter().filter(|v| v.missing()).count(),
                Filter::Missing
            ),
            container(widget::rule::horizontal(1).style(separator)).padding([12, 8]),
            self.navigation_item(
                TAG,
                "标签管理".into(),
                self.snapshot.tags.len(),
                self.page == MainPage::Tags,
                Message::ShowTagManagement
            ),
        ]
        .spacing(4);
        container(
            column![
                container(brand).padding([16, 12]),
                scrollable(navigation)
                    .direction(scrollable::Direction::Vertical(
                        content_scrollbar().spacing(8)
                    ))
                    .height(Fill),
                button(
                    row![
                        icon(FOLDER, 17),
                        text("监控文件夹").size(13).width(Fill),
                        text(self.snapshot.roots.len().to_string())
                            .size(12)
                            .color(colors.muted)
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center)
                )
                .style(quiet)
                .padding(12)
                .width(Fill)
                .on_press(Message::ShowRoots),
                button(label('\u{E713}', "设置"))
                    .style(move |theme, status| item(
                        theme,
                        self.page == MainPage::Settings,
                        status
                    ))
                    .padding(12)
                    .width(Fill)
                    .on_press(Message::ShowSettings),
                button(label(UPDATE, "软件更新"))
                    .style(quiet)
                    .padding(12)
                    .width(Fill)
                    .on_press(Message::ShowUpdates),
            ]
            .spacing(16),
        )
        .padding([8, 8])
        .width(216)
        .height(Fill)
        .into()
    }

    fn library(&self, pending: bool, compact: bool) -> Element<'_, Message> {
        let colors = self.colors();
        let pane = self.pane(pending);
        let videos = self.filtered(pending);
        let total = videos.len();
        let pagination = pane.pagination(total);
        let page = pagination.page;
        let page_videos = &videos[pagination.range()];
        let search = text_input("搜索名称或路径", &pane.search)
            .id(if pending {
                "pending-search"
            } else {
                "library-search"
            })
            .icon(text_input::Icon {
                font: icon_font(),
                code_point: SEARCH,
                size: Some(15.into()),
                spacing: 10.0,
                side: text_input::Side::Left,
            })
            .on_input(move |s| Message::Search(pending, s))
            .size(13)
            .line_height(iced::Pixels(BUTTON_CONTENT_HEIGHT))
            .padding([TOOLBAR_CONTROL_PADDING, 12])
            .style(input);
        let mut controls = row![search];
        if !pending {
            controls = controls.push(self.tag_filter_view());
        }
        let controls = controls
            .push(
                button(button_icon(INFO, 17))
                    .padding(TOOLBAR_CONTROL_PADDING)
                    .style(move |theme, status| {
                        item(theme, self.details_visible[usize::from(pending)], status)
                    })
                    .on_press(Message::ToggleDetails(pending)),
            )
            .spacing(8)
            .align_y(Alignment::Center);
        let mut content = column![
            container(controls).padding(iced::Padding::from(16).bottom(8)),
            self.selection_commands(pending, compact),
        ]
        .spacing(0);
        if self.library_loading {
            content = content.push(
                container(
                    column![
                        motion::spinner(28.0),
                        text("正在打开资料库…").size(13).color(colors.muted)
                    ]
                    .spacing(16)
                    .align_x(Alignment::Center),
                )
                .center_x(Fill)
                .center_y(Fill),
            );
        } else if total == 0 {
            content = content.push(motion::enter(
                (
                    self.filter,
                    &self.tag_filter,
                    &self.excluded_tag_filter,
                    pending,
                ),
                self.empty_state(pending),
            ));
        } else {
            let all_selected = page_videos.iter().all(|v| pane.selected.contains(&v.id));
            content = content.push(
                container(
                    row![
                        checkbox(all_selected)
                            .size(18)
                            .on_toggle(move |_| Message::SelectPage(pending)),
                        self.sort_heading(pending, Sort::Name, Fill),
                        self.sort_heading(pending, Sort::Duration, LIST_DURATION_WIDTH.into()),
                        self.sort_heading(pending, Sort::Size, LIST_SIZE_WIDTH.into()),
                        self.sort_heading(pending, Sort::Modified, LIST_MODIFIED_WIDTH.into()),
                    ]
                    .spacing(LIST_COLUMN_SPACING)
                    .align_y(Alignment::Center),
                )
                .padding(iced::Padding {
                    top: 8.0,
                    bottom: 8.0,
                    left: 18.0,
                    right: 18.0 + CONTENT_SCROLLBAR_SPACE,
                })
                .style(inset),
            );
            let mut list = column![].spacing(2);
            for v in page_videos {
                let video_id = v.id;
                let active = pane.focus == Some(v.id) || pane.selected.contains(&v.id);
                let tag = self.tag_summary(v);
                let copies = v
                    .files
                    .iter()
                    .filter(|f| f.availability == Availability::Present)
                    .count();
                let meta = if v.missing() {
                    format!("{tag}  ·  文件不可访问")
                } else if copies > 1 {
                    format!("{tag}  ·  {copies} 个副本")
                } else {
                    format!("{tag}  ·  {}", extension(v))
                };
                let title = column![
                    list_title(&v.title),
                    text(meta)
                        .size(11)
                        .wrapping(text::Wrapping::Glyph)
                        .color(if v.missing() {
                            colors.danger
                        } else {
                            colors.muted
                        })
                ]
                .spacing(6)
                .width(Fill);
                // Keep names readable at the supported minimum window width.
                let name: Element<_> = if compact {
                    column![self.cover_thumbnail(v), title]
                        .spacing(8)
                        .width(Fill)
                        .into()
                } else {
                    row![self.cover_thumbnail(v), title]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .into()
                };
                let entry = row![
                    checkbox(pane.selected.contains(&v.id))
                        .size(18)
                        .on_toggle(move |on| Message::Check(pending, video_id, on)),
                    container(name).width(Fill).padding([10, 0]),
                    text(listing::duration_label(v.duration_ms))
                        .size(12)
                        .color(colors.muted)
                        .wrapping(text::Wrapping::Glyph)
                        .width(LIST_DURATION_WIDTH),
                    text(bytes_label(v.size()))
                        .size(12)
                        .color(colors.muted)
                        .width(LIST_SIZE_WIDTH),
                    modified_cell(v, pending, colors),
                ]
                .spacing(LIST_COLUMN_SPACING)
                .align_y(Alignment::Center);
                list = list.push(
                    // Child controls consume their own clicks; all remaining
                    // row content and padding select the video.
                    container(
                        button(entry)
                            .width(Fill)
                            .padding([2, 12])
                            .style(quiet)
                            .on_press(Message::Select(pending, v.id)),
                    )
                    .style(move |_| container::Style {
                        background: active.then_some(colors.tint.into()),
                        border: iced::border::rounded(8),
                        ..Default::default()
                    }),
                );
            }
            content = content.push(motion::enter(
                (
                    self.filter,
                    &self.tag_filter,
                    &self.excluded_tag_filter,
                    &pane.search,
                    pane.sort,
                    pane.sort_order,
                    page,
                    pane.page_size,
                ),
                // Reserve a fixed gutter even on short pages, so columns stay
                // aligned with the header when the scrollbar appears/disappears.
                scrollable(container(list).padding(iced::Padding {
                    top: 6.0,
                    bottom: 6.0,
                    left: 6.0,
                    right: 6.0 + CONTENT_SCROLLBAR_SPACE,
                }))
                .id(listing::scroll_id(pending))
                .direction(scrollable::Direction::Vertical(content_scrollbar()))
                .height(Fill),
            ));
        }
        content = content
            .push(widget::rule::horizontal(1).style(separator))
            .push(self.pagination_footer(pending, total));
        container(content)
            .width(Fill)
            .height(Fill)
            .style(panel)
            .into()
    }

    fn sort_heading(&self, pending: bool, sort: Sort, width: Length) -> Element<'_, Message> {
        let pane = self.pane(pending);
        let active = pane.sort == sort;
        let arrow: Element<_> = if active {
            icon(
                if pane.sort_order == SortOrder::Ascending {
                    SORT_ASCENDING
                } else {
                    SORT_DESCENDING
                },
                10,
            )
            .width(12)
            .into()
        } else {
            space::horizontal().width(12).into()
        };
        button(
            row![text(sort.label()).size(12), arrow]
                .spacing(4)
                .align_y(Alignment::Center),
        )
        .width(width)
        .padding([6, 4])
        .style(move |theme, status| sort_header(theme, active, status))
        .on_press(Message::Sort(pending, sort))
        .into()
    }

    fn pagination_footer(&self, pending: bool, total: usize) -> Element<'_, Message> {
        let pane = self.pane(pending);
        let colors = self.colors();
        let pagination = pane.pagination(total);
        let page = pagination.page;
        let pages = pagination.pages;
        let enabled = total > 0 && !self.library_loading;
        let previous = enabled && page > 0;
        let next = enabled && page + 1 < pages;
        let target = pane.jump_target(total);
        let invalid = pane.page_input.is_some() && target.is_none();
        let summary = if invalid {
            format!("请输入 1–{pages} 之间的页码")
        } else if total == 0 {
            "共 0 个视频".into()
        } else {
            format!(
                "{}–{} / 共 {total} 个",
                pagination.start + 1,
                pagination.end
            )
        };
        let page_text = pane
            .page_input
            .clone()
            .unwrap_or_else(|| (page + 1).to_string());
        let jump = (enabled && target.is_some_and(|target| target != page))
            .then_some(Message::JumpPage(pending));
        let input = text_input("页码", &page_text)
            .id(if pending {
                "pending-page"
            } else {
                "library-page"
            })
            .on_input_maybe(enabled.then_some(move |value| Message::PageInput(pending, value)))
            .on_submit_maybe(jump.clone())
            .size(12)
            .padding([6, 8])
            .width(56)
            .style(input);
        let navigation = row![
            button(button_text("首页").size(12))
                .padding([6, 8])
                .style(quiet)
                .on_press_maybe(previous.then_some(Message::Page(pending, 0))),
            page_arrow(
                '\u{E76B}',
                previous.then_some(Message::Page(pending, page.saturating_sub(1)))
            ),
            space::horizontal(),
            text("第").size(12).color(colors.muted),
            widget::tooltip(
                input,
                "输入页码，按 Enter 跳转",
                widget::tooltip::Position::Top
            )
            .style(panel)
            .padding(8),
            text(format!("/ {pages} 页")).size(12).color(colors.muted),
            button(button_text("跳转").size(12))
                .padding([6, 8])
                .style(quiet)
                .on_press_maybe(jump),
            space::horizontal(),
            page_arrow('\u{E76C}', next.then_some(Message::Page(pending, page + 1))),
            button(button_text("末页").size(12))
                .padding([6, 8])
                .style(quiet)
                .on_press_maybe(next.then_some(Message::Page(pending, pages - 1))),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        container(
            column![
                row![
                    text(summary)
                        .size(12)
                        .color(if invalid { colors.danger } else { colors.muted })
                        .width(Fill),
                    pick_list(PageSize::ALL, Some(pane.page_size), move |size| {
                        Message::PageSize(pending, size)
                    })
                    .style(select)
                    .menu_style(menu)
                    .text_size(12)
                    .padding([5, 8]),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                navigation,
            ]
            .spacing(6),
        )
        .padding([8, 16])
        .into()
    }

    fn empty_state(&self, pending: bool) -> Element<'_, Message> {
        let colors = self.colors();
        let searching = !self.pane(pending).search.is_empty();
        let filtering_tags = !pending && self.has_tag_filter();
        let (glyph, title, description) = if filtering_tags {
            (
                TAG,
                "没有符合条件的视频",
                "当前分类和搜索结果中，没有符合标签包含与排除条件的视频。",
            )
        } else if searching {
            (
                SEARCH,
                "没有找到匹配的视频",
                "试试其他名称，或输入文件路径的一部分。",
            )
        } else if pending {
            (
                CHECK,
                "所有视频都已整理",
                "导入视频或监控文件夹后，新内容会出现在这里。",
            )
        } else {
            match self.filter {
                Filter::All => (
                    LIBRARY,
                    "你的影像，从这里开始",
                    "导入本地视频，或添加文件夹自动发现新内容。",
                ),
                Filter::Untagged => (
                    TAG,
                    "所有视频都已有标签",
                    "新导入但尚无标签的视频会显示在这里。",
                ),
                Filter::Duplicates => (
                    COPY,
                    "没有重复副本",
                    "相同内容的多个文件会自动合并到一个条目。",
                ),
                Filter::Missing => (
                    CHECK,
                    "所有文件均可访问",
                    "文件离线或无法访问时，你可以在这里查看。",
                ),
            }
        };
        let mut empty = column![
            container(icon(glyph, 36).color(colors.accent))
                .center_x(88)
                .center_y(80)
                .style(move |_| container::Style {
                    background: Some(colors.tint.into()),
                    border: iced::border::rounded(16),
                    ..Default::default()
                }),
            space::vertical().height(4),
            heading(title, 21),
            text(description)
                .size(13)
                .color(colors.muted)
                .align_x(Alignment::Center),
        ]
        .spacing(14)
        .align_x(Alignment::Center);
        if filtering_tags {
            empty = empty.push(
                button(button_text("清除标签筛选").size(13))
                    .style(secondary)
                    .padding([10, 20])
                    .on_press(Message::ClearTagFilter),
            );
        }
        if searching {
            empty = empty.push(
                button(button_text("清除搜索").size(13))
                    .style(secondary)
                    .padding([10, 20])
                    .on_press(Message::Search(pending, String::new())),
            );
        } else if !pending && !filtering_tags && self.filter == Filter::All {
            empty = empty
                .push(
                    button(label(ADD, "导入第一个视频"))
                        .padding([11, 20])
                        .style(primary)
                        .on_press(Message::Import),
                )
                .push(
                    text("支持 MP4、MKV、MOV 等常见格式")
                        .size(11)
                        .color(colors.muted),
                );
        }
        container(empty)
            .padding(24)
            .center_x(Fill)
            .center_y(Fill)
            .into()
    }

    fn details(&self, pending: bool, id: window::Id) -> Element<'_, Message> {
        let colors = self.colors();
        let focus = self
            .pane(pending)
            .focus
            .and_then(|id| self.snapshot.videos.iter().find(|v| v.id == id));
        let header = row![
            heading("视频详情", 15).width(Fill),
            button(button_icon(CLOSE, 12))
                .style(quiet)
                .padding(8)
                .on_press(Message::ToggleDetails(pending)),
        ]
        .align_y(Alignment::Center);
        let mut content = column![self.player_view(id, false)].spacing(20);
        if let Some(v) = focus {
            let names = self.tag_names(v);
            let tags = if names.is_empty() {
                "无标签".into()
            } else {
                names.join("、")
            };

            let mut video_info_column = column![
                detail_row(self.colors(), "文件类型", &extension(v)),
                detail_row(self.colors(), "文件大小", &bytes_label(v.size())),
                detail_row(self.colors(), "标签", &tags),
            ]
            .spacing(12);

            if v.files.len() > 1 {
                video_info_column = video_info_column.push(detail_row(
                    self.colors(),
                    "文件位置",
                    &format!("{} 个", v.files.len()),
                ));
            }

            content = content
                .push(heading(v.title.clone(), 17).wrapping(text::Wrapping::Glyph))
                .push(video_info_column)
                .push(widget::rule::horizontal(1).style(separator))
                .push(
                    row![
                        heading("文件位置", 13).width(Fill),
                        badge(self.colors(), v.files.len().to_string(), false)
                    ]
                    .align_y(Alignment::Center),
                );
            for file in &v.files {
                content = content.push(
                    container(
                        column![
                            text(file.path.to_string_lossy().to_string())
                                .size(12)
                                .wrapping(text::Wrapping::Glyph),
                            row![
                                button(label(COPY, "复制路径"))
                                    .style(quiet)
                                    .padding(6)
                                    .on_press(Message::Copy(
                                        file.path.to_string_lossy().to_string()
                                    )),
                                space::horizontal(),
                                button(label(FOLDER, "在资源管理器打开"))
                                    .style(quiet)
                                    .padding(6)
                                    .on_press_maybe(
                                        (file.availability == Availability::Present)
                                            .then(|| Message::OpenInExplorer(file.path.clone()))
                                    ),
                            ]
                            .align_y(Alignment::Center)
                        ]
                        .spacing(10),
                    )
                    .padding(12)
                    .style(inset),
                );
            }
            if v.files.is_empty() {
                content = content.push(
                    text("原路径内容已变化，找到相同内容后会重新关联。")
                        .size(12)
                        .color(colors.muted),
                );
            }
            content = content
                .push(widget::rule::horizontal(1).style(separator))
                .push(
                    column![
                        row![
                            heading("内容指纹", 13).width(Fill),
                            button(button_text("复制").size(12))
                                .style(quiet)
                                .padding(6)
                                .on_press(Message::Copy(v.hash.clone()))
                        ]
                        .align_y(Alignment::Center),
                        text("SHA-256 · 用于识别相同内容")
                            .size(11)
                            .color(colors.muted),
                        text(&v.hash)
                            .size(11)
                            .color(colors.muted)
                            .wrapping(text::Wrapping::Glyph),
                    ]
                    .spacing(8),
                );
        } else {
            content = content.push(
                container(
                    column![
                        heading("先选择一个视频", 16),
                        text("在列表中选择视频，即可查看文件信息、标签和所有存储位置。")
                            .size(13)
                            .color(colors.muted),
                        widget::rule::horizontal(1).style(separator),
                        row![
                            icon(INFO, 15).color(colors.muted),
                            text("相同内容自动合并，\n每个文件位置都会保留。")
                                .size(12)
                                .color(colors.muted)
                        ]
                        .spacing(10)
                    ]
                    .spacing(16),
                )
                .padding([8, 0]),
            );
        }
        let inspector = scrollable(content)
            .direction(scrollable::Direction::Vertical(
                content_scrollbar().spacing(8),
            ))
            .height(Fill);
        container(column![header, inspector].spacing(16))
            .padding(16)
            .width(Fill)
            .height(Fill)
            .style(panel)
            .into()
    }

    fn cover_thumbnail(&self, video: &VideoItem) -> Element<'_, Message> {
        let colors = self.colors();
        let state = self.covers.state(&video.hash);
        let content: Element<_> = match state {
            Some(covers::State::Ready(handle)) => widget::image(handle.clone())
                .width(72)
                .height(44)
                .content_fit(iced::ContentFit::Contain)
                .into(),
            _ => icon(VIDEO, 20)
                .color(if video.missing() {
                    colors.muted
                } else {
                    colors.accent
                })
                .into(),
        };
        container(content)
            .center_x(72)
            .center_y(44)
            .style(move |_| container::Style {
                background: Some(colors.backdrop.into()),
                border: iced::border::rounded(8),
                ..Default::default()
            })
            .into()
    }

    fn unloaded_player(
        &self,
        video: &VideoItem,
        pending: bool,
        id: window::Id,
    ) -> Element<'_, Message> {
        let colors = self.colors();
        let playable = self.preview_file(pending, video.id);
        column![
            container(
                column![
                    icon(VIDEO, 24).color(colors.muted),
                    text(if playable.is_some() {
                        "点击下方播放按钮加载视频"
                    } else {
                        "文件不可用，请检查存储位置"
                    })
                    .size(12)
                    .color(colors.muted),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center_x(Fill)
            .center_y(170)
            .style(inset),
            button(button_icon(PLAY, 16))
                .style(secondary)
                .padding(8)
                .on_press_maybe(playable.map(|file| Message::Play(file, id))),
        ]
        .spacing(12)
        .into()
    }

    fn player_view(&self, id: window::Id, full: bool) -> Element<'_, Message> {
        let colors = self.colors();
        if id != self.player_window || (self.player.is_none() && self.loader.is_none()) {
            let pending = Some(id) == self.pending;
            if let Some(video) = self
                .pane(pending)
                .focus
                .and_then(|focus| self.snapshot.videos.iter().find(|v| v.id == focus))
            {
                return self.unloaded_player(video, pending, id);
            }
            return container(
                column![
                    container(icon(PLAY, 24).color(colors.accent))
                        .center_x(52)
                        .center_y(52)
                        .style(move |_| container::Style {
                            background: Some(colors.surface.into()),
                            border: iced::border::rounded(26),
                            ..Default::default()
                        }),
                    text("视频预览").size(13).color(colors.muted),
                    text("选择视频后，在这里观看").size(11).color(colors.muted),
                ]
                .spacing(10)
                .align_x(Alignment::Center),
            )
            .center_x(Fill)
            .center_y(if full { Fill } else { Length::Fixed(170.0) })
            .style(move |_| container::Style {
                background: Some(colors.tint.into()),
                border: iced::border::rounded(10),
                ..Default::default()
            })
            .into();
        }
        let Some(video) = &self.player else {
            return container(
                column![
                    motion::spinner(28.0),
                    text("正在加载视频…").size(13).color(colors.muted),
                    button(button_text("取消").size(12))
                        .style(quiet)
                        .on_press(Message::Stop)
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center_x(Fill)
            .center_y(170)
            .style(inset)
            .into();
        };
        let display = VideoPlayer::new(video)
            .width(Fill)
            .height(if full { Fill } else { Length::Fixed(170.0) })
            .content_fit(iced::ContentFit::Contain)
            .on_new_frame(Message::Frame);
        column![
            container(display)
                .width(Fill)
                .height(if full { Fill } else { Length::Fixed(170.0) })
                .style(move |_| container::Style {
                    background: Some(Color::BLACK.into()),
                    border: iced::border::rounded(8),
                    ..Default::default()
                }),
            text(&self.player_title)
                .size(12)
                .wrapping(text::Wrapping::Glyph),
            slider(
                0.0..=video.duration().as_secs_f64().max(0.1),
                self.position,
                Message::Seek
            )
            .step(0.1)
            .on_release(Message::SeekRelease),
            row![
                button(button_icon(
                    if video.paused() { PLAY } else { '\u{E769}' },
                    16
                ))
                .padding(8)
                .style(secondary)
                .on_press(Message::Pause),
                text(format!(
                    "{} / {}",
                    time_label(self.position),
                    time_label(video.duration().as_secs_f64())
                ))
                .size(11)
                .color(colors.muted),
                space::horizontal(),
                button(button_icon(if full { '\u{E73F}' } else { '\u{E740}' }, 15))
                    .padding(8)
                    .style(quiet)
                    .on_press(Message::Fullscreen(id)),
                button(button_icon('\u{E71A}', 14))
                    .padding(8)
                    .style(quiet)
                    .on_press(Message::Stop),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                button(button_icon(
                    if self.settings.muted {
                        '\u{E74F}'
                    } else {
                        '\u{E767}'
                    },
                    16
                ))
                .padding(8)
                .style(quiet)
                .on_press(Message::Mute),
                slider(0.0..=1.0, self.settings.volume, Message::Volume).step(0.01),
                text(format!("{}%", (self.settings.volume * 100.0).round()))
                    .size(11)
                    .color(colors.muted),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(12)
        .into()
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let colors = self.colors();
        let activity: Element<'_, Message> = if self.progress.is_some() || self.library_loading {
            motion::spinner(14.0)
        } else {
            icon(CHECK, 12)
                .color(if self.error.is_some() {
                    colors.muted
                } else {
                    colors.success
                })
                .into()
        };
        let mut bar = row![
            activity,
            text(&self.status).size(11).color(colors.muted).width(Fill)
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some((done, total, bytes)) = self.progress {
            bar = bar
                .push(
                    text(if total == 0 {
                        "正在查找视频…".into()
                    } else {
                        format!("{done}/{total} · {}", bytes_label(bytes))
                    })
                    .size(11)
                    .color(colors.muted),
                )
                .push(
                    progress_bar(0.0..=total.max(1) as f32, done as f32)
                        .length(88)
                        .girth(3),
                )
                .push(
                    button(button_text("取消").size(11))
                        .padding([4, 8])
                        .style(quiet)
                        .on_press(Message::CancelScan),
                );
        }
        bar = bar
            .push(
                button(label(REFRESH, "扫描"))
                    .style(quiet)
                    .padding([4, 8])
                    .on_press_maybe(self.progress.is_none().then_some(Message::Scan(false))),
            )
            .push(
                button(button_text("完整校验").size(12))
                    .style(quiet)
                    .padding([4, 8])
                    .on_press_maybe(self.progress.is_none().then_some(Message::Scan(true))),
            );
        container(bar)
            .padding([6, 20])
            .style(move |_| container::Style {
                background: Some(colors.subtle.into()),
                ..Default::default()
            })
            .into()
    }

    fn update_dialog(&self, available: bool) -> Element<'_, Message> {
        let colors = self.colors();
        let state = &self.updates;
        let can_configure = state.can_configure() && self.service.is_some();
        let idle = state.phase == updates::Phase::Idle && !state.saving;
        let folder = state
            .settings
            .folder
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "尚未选择更新文件夹".into());
        let mut content = column![
            heading(
                if available {
                    "发现新版本"
                } else {
                    "软件更新"
                },
                22
            ),
            text(format!("当前版本 {}", video_management::update::VERSION))
                .size(13)
                .color(colors.muted),
            container(
                column![
                    text("更新文件夹").size(12).color(colors.muted),
                    text(folder).size(13).wrapping(text::Wrapping::Glyph),
                ]
                .spacing(8)
            )
            .width(Fill)
            .padding(14)
            .style(inset),
        ]
        .spacing(16);
        if !available {
            content = content.push(
                row![
                    button(label(FOLDER, "选择文件夹"))
                        .padding([10, 16])
                        .style(secondary)
                        .on_press_maybe(can_configure.then_some(Message::PickUpdateFolder)),
                    button(button_text("清除").size(13))
                        .padding([10, 16])
                        .style(quiet)
                        .on_press_maybe(
                            (can_configure && state.settings.folder.is_some())
                                .then_some(Message::ClearUpdateFolder)
                        ),
                    space::horizontal(),
                    button(button_text("检查更新").size(13))
                        .padding([10, 16])
                        .style(secondary)
                        .on_press_maybe(
                            (can_configure && idle && state.settings.folder.is_some())
                                .then_some(Message::CheckUpdate)
                        ),
                ]
                .spacing(8),
            );
        }
        if let Some(candidate) = &state.candidate {
            content = content.push(
                container(
                    column![
                        text(format!("新版本 {}", candidate.release.version))
                            .size(17)
                            .color(colors.accent),
                        text(format!(
                            "{} · {}",
                            candidate.release.archive,
                            bytes_label(candidate.release.size)
                        ))
                        .size(12)
                        .color(colors.muted)
                        .wrapping(text::Wrapping::Glyph),
                        text("更新将关闭程序并自动重启，资料库和设置会保留。")
                            .size(13)
                            .wrapping(text::Wrapping::Glyph),
                    ]
                    .spacing(8),
                )
                .padding(14)
                .width(Fill)
                .style(inset),
            );
        }
        let activity: Element<'_, Message> = if !idle && !state.failed {
            motion::spinner(18.0)
        } else {
            icon(if state.failed { WARNING } else { INFO }, 16)
                .color(if state.failed {
                    colors.danger
                } else {
                    colors.accent
                })
                .into()
        };
        content = content.push(
            row![
                activity,
                text(&state.status)
                    .size(13)
                    .width(Fill)
                    .wrapping(text::Wrapping::Glyph)
                    .color(if state.failed {
                        colors.danger
                    } else {
                        colors.muted
                    }),
            ]
            .spacing(10),
        );
        content = content.push(
            text("仅检查所选文件夹中的完整正式发布包。文件夹不可用时仍可正常使用资料库。")
                .size(12)
                .color(colors.muted)
                .wrapping(text::Wrapping::Glyph),
        );
        let mut buttons = row![
            space::horizontal(),
            button(
                button_text(if state.candidate.is_some() {
                    "稍后"
                } else {
                    "完成"
                })
                .size(13)
            )
            .padding([10, 16])
            .width(100)
            .style(secondary)
            .on_press_maybe((!state.installing() && !state.saving).then_some(Message::Dismiss)),
        ]
        .spacing(12);
        if state.candidate.is_some() {
            buttons = buttons.push(
                button(label(UPDATE, "立即更新"))
                    .padding([10, 16])
                    .style(primary)
                    .on_press_maybe(
                        (idle && self.service.is_some()).then_some(Message::InstallUpdate),
                    ),
            );
        }
        column![
            scrollable(content).height(Length::Shrink),
            widget::rule::horizontal(1).style(separator),
            buttons
        ]
        .spacing(20)
        .into()
    }

    fn dialog_view<'a>(&'a self, dialog: &'a Dialog) -> Element<'a, Message> {
        let colors = self.colors();
        let cancel = || {
            button(button_text("取消").size(13))
                .width(100)
                .padding([10, 16])
                .style(secondary)
                .on_press(Message::Dismiss)
        };
        match dialog {
            Dialog::Tags(picker) => self.tag_picker_view(picker),
            Dialog::Updates | Dialog::UpdateAvailable => {
                self.update_dialog(matches!(dialog, Dialog::UpdateAvailable))
            }
            Dialog::Tag(edit) => column![
                heading(
                    if edit.is_some() {
                        "重命名标签"
                    } else {
                        "新建标签"
                    },
                    22
                ),
                text("把相关视频整理到一起，原文件仍保留在原位置。")
                    .size(13)
                    .color(colors.muted),
                column![
                    text("标签名称").size(13),
                    text_input("例如：旅行记录、工作素材", &self.tag_name)
                        .id("tag-name")
                        .size(14)
                        .padding(12)
                        .style(input)
                        .on_input(Message::TagName)
                        .on_submit(Message::SaveTag),
                    text("最多 80 个字符").size(11).color(colors.muted)
                ]
                .spacing(8),
                widget::rule::horizontal(1).style(separator),
                row![
                    space::horizontal(),
                    cancel(),
                    button(button_text("保存").size(13))
                        .width(100)
                        .padding([10, 16])
                        .style(primary)
                        .on_press_maybe(
                            (!self.tag_name.trim().is_empty()
                                && self.tag_name.trim().chars().count() <= 80)
                                .then_some(Message::SaveTag)
                        )
                ]
                .spacing(8),
            ]
            .spacing(20)
            .into(),
            Dialog::DeleteTag(id) => column![
                heading("删除这个标签？", 22),
                text(
                    self.snapshot
                        .tags
                        .iter()
                        .find(|tag| tag.id == *id)
                        .map(|tag| tag.name.as_str())
                        .unwrap_or("标签已不存在")
                )
                .size(16)
                .wrapping(text::Wrapping::Glyph),
                text("将从所有视频中移除此标签，其他标签和磁盘上的原文件会保留。").size(14),
                widget::rule::horizontal(1).style(separator),
                row![
                    space::horizontal(),
                    cancel(),
                    button(button_text("删除标签").size(13))
                        .style(danger)
                        .padding([10, 16])
                        .on_press(Message::ConfirmDeleteTag)
                ]
                .spacing(8)
            ]
            .spacing(24)
            .into(),
            Dialog::Assign { .. } => column![
                heading("相同内容共享标签", 22),
                text("所选文件中包含已有视频的副本。更改标签会同时影响已有视频及其全部副本。")
                    .size(14),
                widget::rule::horizontal(1).style(separator),
                row![
                    space::horizontal(),
                    cancel(),
                    button(button_text("确认导入并更改标签").size(13))
                        .style(primary)
                        .padding([10, 16])
                        .on_press(Message::ConfirmAssign)
                ]
                .spacing(8)
            ]
            .spacing(24)
            .into(),
            Dialog::Trash {
                files, selected, ..
            } => {
                let mut paths = column![].spacing(12);
                for file in files {
                    let file_id = file.id;
                    paths = paths.push(
                        container(
                            row![
                                checkbox(selected.contains(&file.id))
                                    .size(18)
                                    .on_toggle(move |on| Message::TrashCheck(file_id, on)),
                                column![
                                    text(file.path.to_string_lossy().to_string())
                                        .size(12)
                                        .wrapping(text::Wrapping::Glyph),
                                    text(format!(
                                        "{} · {}",
                                        bytes_label(file.size),
                                        file.availability.label()
                                    ))
                                    .size(11)
                                    .color(colors.muted)
                                ]
                                .spacing(6)
                                .width(Fill)
                            ]
                            .spacing(12)
                            .align_y(Alignment::Center),
                        )
                        .padding(12)
                        .style(inset),
                    );
                }
                column![
                    heading("将文件移入回收站？", 22),
                    text("请核对文件路径。默认每个视频仅选一个可访问副本，未选中的文件会保留。")
                        .size(13)
                        .color(colors.muted),
                    scrollable(paths).height(260),
                    text(format!("已选择 {} 个文件", selected.len()))
                        .size(12)
                        .color(colors.muted),
                    widget::rule::horizontal(1).style(separator),
                    row![
                        space::horizontal(),
                        cancel(),
                        button(button_text("移入回收站").size(13))
                            .style(danger)
                            .padding([10, 16])
                            .on_press_maybe(
                                (!selected.is_empty()).then_some(Message::ConfirmTrash)
                            )
                    ]
                    .spacing(8)
                ]
                .spacing(20)
                .into()
            }
            Dialog::Roots => {
                let mut paths = column![].spacing(12);
                for root in &self.snapshot.roots {
                    paths = paths.push(
                        container(
                            column![
                                row![
                                    icon(FOLDER, 16).color(colors.accent),
                                    text(root.to_string_lossy().to_string())
                                        .size(13)
                                        .wrapping(text::Wrapping::Glyph)
                                        .width(Fill)
                                ]
                                .spacing(12),
                                row![
                                    text("监控中").size(11).color(colors.success),
                                    space::horizontal(),
                                    button(button_text("停止监控").size(12))
                                        .style(quiet)
                                        .padding(6)
                                        .on_press(Message::RemoveRoot(root.clone()))
                                ]
                                .align_y(Alignment::Center)
                            ]
                            .spacing(8),
                        )
                        .padding(12)
                        .style(inset),
                    );
                }
                if self.snapshot.roots.is_empty() {
                    paths = paths.push(
                        container(
                            column![
                                icon(FOLDER, 28).color(colors.accent),
                                text("还没有监控文件夹").size(15),
                                text("添加后，文件夹中的新视频会自动进入待处理列表。")
                                    .size(12)
                                    .color(colors.muted)
                            ]
                            .spacing(12)
                            .align_x(Alignment::Center),
                        )
                        .padding(24)
                        .center_x(Fill),
                    );
                }
                column![
                    heading("监控文件夹", 22),
                    text("自动发现子文件夹中的新视频，每 60 秒检查一次。")
                        .size(13)
                        .color(colors.muted),
                    scrollable(paths).height(240),
                    text(format!("资料库位置：{}", self.data.display()))
                        .size(11)
                        .color(colors.muted)
                        .wrapping(text::Wrapping::Glyph),
                    widget::rule::horizontal(1).style(separator),
                    row![
                        button(label(ADD, "添加文件夹"))
                            .style(secondary)
                            .padding([10, 16])
                            .on_press(Message::PickRoots),
                        space::horizontal(),
                        button(button_text("完成").size(13))
                            .width(100)
                            .style(primary)
                            .padding([10, 16])
                            .on_press(Message::Dismiss)
                    ]
                ]
                .spacing(20)
                .into()
            }
        }
    }
}

pub(super) fn badge<'a>(colors: Palette, value: String, accent: bool) -> Element<'a, Message> {
    container(
        text(value)
            .size(11)
            .color(if accent { colors.accent } else { colors.muted }),
    )
    .padding([3, 8])
    .style(move |_| container::Style {
        background: Some(if accent { colors.tint } else { colors.subtle }.into()),
        border: iced::border::rounded(10),
        ..Default::default()
    })
    .into()
}

fn detail_row<'a>(colors: Palette, label: &str, value: &str) -> Element<'a, Message> {
    row![
        text(label.to_owned())
            .size(12)
            .color(colors.muted)
            .width(50),
        text(value.to_owned())
            .size(12)
            .wrapping(text::Wrapping::Glyph)
            .width(Fill)
    ]
    .spacing(12)
    .into()
}

fn extension(video: &VideoItem) -> String {
    video
        .files
        .first()
        .and_then(|file| file.path.extension())
        .map(|value| value.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "视频".into())
}

fn list_title<'a>(title: &'a str) -> Element<'a, Message> {
    let title = title_without_extension(title);
    widget::responsive(move |size| {
        text(truncate_title(&title, size.width))
            .size(14)
            .wrapping(text::Wrapping::None)
            .width(Fill)
            .into()
    })
    .height(Length::Shrink)
    .into()
}

fn title_without_extension(title: &str) -> String {
    Path::new(title)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| title.to_owned())
}

fn truncate_title(title: &str, max_width: f32) -> String {
    if title.is_empty() || title_width(title) <= max_width {
        return title.to_owned();
    }

    let ellipsis = '…';
    let available = (max_width - title_char_width(ellipsis)).max(0.0);
    let mut width = 0.0;
    let mut result = String::new();
    for character in title.chars() {
        let character_width = title_char_width(character);
        if width + character_width > available {
            break;
        }
        result.push(character);
        width += character_width;
    }
    result.push(ellipsis);
    result
}

fn title_width(title: &str) -> f32 {
    title.chars().map(title_char_width).sum()
}

fn title_char_width(character: char) -> f32 {
    if character.is_ascii() { 7.0 } else { 14.0 }
}

#[cfg(test)]
mod tests {
    use super::{title_without_extension, truncate_title};

    #[test]
    fn list_title_removes_only_the_final_extension() {
        assert_eq!(title_without_extension("旅行记录.mp4"), "旅行记录");
        assert_eq!(title_without_extension("片段.part.mkv"), "片段.part");
        assert_eq!(title_without_extension("没有扩展名"), "没有扩展名");
    }

    #[test]
    fn list_title_uses_a_single_line_ellipsis_when_it_does_not_fit() {
        assert_eq!(truncate_title("短标题", 100.0), "短标题");
        assert_eq!(truncate_title("这是一个很长的视频名称", 70.0), "这是一个…");
    }
}
