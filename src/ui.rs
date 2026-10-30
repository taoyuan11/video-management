use iced::{
    Alignment, Color, Element, Fill, Length, Subscription, Task,
    widget::{
        self, checkbox, container, pick_list, progress_bar, row, scrollable, slider, space, stack,
        text, text_input,
    },
    window,
};
use iced_video_player::{Video, VideoPlayer};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};
use video_management::{
    model::*,
    scanner::VIDEO_EXTENSIONS,
    service::{Command, Event, Service},
};
mod backdrop;
mod branding;
mod covers;
mod dropdown;
pub(crate) mod fluent;
mod listing;
mod motion;
mod playback;
mod resize;
mod settings;
mod tag_filter;
mod tag_management;
mod tags;
mod updates;
mod views;
use fluent::*;
use listing::{PageSize, Sort, SortOrder};
use motion::button;
use tags::{TagAction, TagPicker};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    All,
    Untagged,
    Duplicates,
    Missing,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainPage {
    Library,
    Tags,
    Settings,
}
#[derive(Default)]
struct Pane {
    search: String,
    sort: Sort,
    sort_order: SortOrder,
    selected: BTreeSet<i64>,
    focus: Option<i64>,
    page: usize,
    page_size: PageSize,
    page_input: Option<String>,
    tag_draft: BTreeSet<i64>,
}
#[derive(Debug, Clone)]
enum Dialog {
    Tag(Option<i64>),
    DeleteTag(i64),
    Tags(TagPicker),
    Trash {
        files: Vec<VideoFile>,
        selected: BTreeSet<i64>,
        targets: Vec<DeleteTarget>,
    },
    Assign {
        ids: Vec<i64>,
        assignment: TagAssignment,
        accept: bool,
    },
    Roots,
    Updates,
    UpdateAvailable,
}
#[derive(Debug, Clone)]
pub enum Message {
    WindowOpened(window::Id),
    BackdropReady(window::Id, bool, Result<(), String>),
    ShowSettings,
    ShowTagManagement,
    ShowLibrary,
    SettingFocus(usize),
    NavigationKey(window::Id, iced::keyboard::key::Named, bool),
    Concurrency(usize),
    Theme(ThemePreference),
    Tick,
    Close(window::Id),
    OpenPending,
    Filter(Filter),
    Import,
    PickRoots,
    Files(Vec<PathBuf>),
    Roots(Vec<PathBuf>),
    Scan(bool),
    CancelScan,
    Search(bool, String),
    TagFilterOpen(bool),
    TagFilterCheck(i64, bool),
    ClearTagFilter,
    TagFilterKey(iced::keyboard::key::Named, bool),
    Sort(bool, Sort),
    Select(bool, i64),
    Check(bool, i64, bool),
    SelectPage(bool),
    Page(bool, usize),
    PageSize(bool, PageSize),
    PageInput(bool, String),
    JumpPage(bool),
    PickTags(TagAction),
    TagCheck(i64, bool),
    SaveTagSelection,
    Assign(bool),
    ConfirmAssign,
    Forget(bool),
    NewTag(window::Id),
    EditTag(i64),
    DeleteTag(i64),
    TagName(String),
    SaveTag,
    ConfirmDeleteTag,
    Trash(bool, window::Id),
    TrashCheck(i64, bool),
    ConfirmTrash,
    Dismiss,
    ShowRoots,
    ShowUpdates,
    PickUpdateFolder,
    UpdateFolder(Option<PathBuf>),
    ClearUpdateFolder,
    CheckUpdate,
    InstallUpdate,
    RemoveRoot(PathBuf),
    Play(i64, window::Id),
    Pause,
    Seek(f64),
    SeekRelease,
    Volume(f64),
    Mute,
    Fullscreen(window::Id),
    Frame,
    Stop,
    Copy(String),
    OpenInExplorer(PathBuf),
    ClearError,
    ClearUpdateNotice,
    ToggleDetails(bool),
    ResizeDetails(bool, f32),
    ClearSelection(bool),
    FocusSearch(window::Id),
}
pub struct App {
    main: window::Id,
    pending: Option<window::Id>,
    service: Option<Service>,
    snapshot: Snapshot,
    filter: Filter,
    tag_filter: BTreeSet<i64>,
    tag_filter_open: bool,
    tag_filter_focus: usize,
    main_pane: Pane,
    pending_pane: Pane,
    dialog: Option<(window::Id, Dialog)>,
    tag_name: String,
    status: String,
    error: Option<String>,
    progress: Option<(usize, usize, u64)>,
    library_loading: bool,
    motion_checked: std::time::Instant,
    data: PathBuf,
    covers: covers::Covers,
    player: Option<Video>,
    loader: Option<playback::Load>,
    playback_jobs: playback::Jobs,
    pending_trash: Option<(Vec<DeleteTarget>, std::time::Instant)>,
    playing_file: Option<i64>,
    player_window: window::Id,
    player_title: String,
    position: f64,
    dragging: bool,
    resume_after_seek: bool,
    fullscreen: bool,
    settings: Settings,
    app_settings: AppSettings,
    app_settings_loaded: bool,
    settings_saving: Option<AppSettings>,
    page: MainPage,
    tag_management_focus: usize,
    setting_focus: usize,
    system_dark: bool,
    native_dark: Option<bool>,
    details_visible: [bool; 2],
    details_width: [f32; 2],
    material_windows: BTreeSet<window::Id>,
    updates: updates::UpdateUi,
    update_notice: Option<String>,
}
impl App {
    pub fn boot(
        data: PathBuf,
        start_error: Option<String>,
        update_result: Option<String>,
    ) -> (Self, Task<Message>) {
        motion::refresh_preferences();
        let (main, open) = window::open(window::Settings {
            icon: Some(branding::window_icon()),
            size: iced::Size::new(1380.0, 860.0),
            min_size: Some(iced::Size::new(1060.0, 700.0)),
            position: window::Position::Centered,
            exit_on_close_request: false,
            transparent: true,
            ..Default::default()
        });
        let (service, error) = match Service::start(data.clone()) {
            Ok(s) => (Some(s), start_error),
            Err(e) => (None, Some(format!("{e:#}"))),
        };
        let library_loading = service.is_some();
        (
            Self {
                main,
                pending: None,
                service,
                snapshot: Snapshot::default(),
                filter: Filter::All,
                tag_filter: BTreeSet::new(),
                tag_filter_open: false,
                tag_filter_focus: 0,
                main_pane: Pane::default(),
                pending_pane: Pane::default(),
                dialog: None,
                tag_name: String::new(),
                status: "正在打开资料库…".into(),
                error,
                progress: None,
                library_loading,
                motion_checked: std::time::Instant::now(),
                data,
                covers: covers::Covers::default(),
                player: None,
                loader: None,
                playback_jobs: playback::Jobs::default(),
                pending_trash: None,
                playing_file: None,
                player_window: main,
                player_title: String::new(),
                position: 0.0,
                dragging: false,
                resume_after_seek: false,
                fullscreen: false,
                settings: Settings::default(),
                app_settings: AppSettings::default(),
                app_settings_loaded: false,
                settings_saving: None,
                page: MainPage::Library,
                tag_management_focus: 1,
                setting_focus: 0,
                system_dark: settings::system_dark().unwrap_or(false),
                native_dark: None,
                details_visible: [true, true],
                details_width: [DETAILS_DEFAULT_WIDTH; 2],
                material_windows: BTreeSet::new(),
                updates: updates::UpdateUi::default(),
                update_notice: update_result,
            },
            open.map(Message::WindowOpened),
        )
    }
    pub fn title(&self, id: window::Id) -> String {
        if Some(id) == self.pending {
            "待处理视频 · 帧库".into()
        } else {
            "帧库 · 本地视频管理".into()
        }
    }
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
            window::close_requests().map(Message::Close),
            iced::event::listen_with(|event, status, window| {
                if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Character(key),
                    modifiers,
                    ..
                }) = &event
                    && modifiers.control()
                {
                    if key.eq_ignore_ascii_case("f") {
                        return Some(Message::FocusSearch(window));
                    }
                    if key.eq_ignore_ascii_case("o") {
                        return Some(Message::Import);
                    }
                }
                if status == iced::event::Status::Ignored
                    && let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                        key: iced::keyboard::Key::Named(key),
                        modifiers,
                        ..
                    }) = &event
                    && !modifiers.control()
                    && !modifiers.alt()
                    && matches!(
                        key,
                        iced::keyboard::key::Named::Tab
                            | iced::keyboard::key::Named::ArrowUp
                            | iced::keyboard::key::Named::ArrowDown
                            | iced::keyboard::key::Named::Home
                            | iced::keyboard::key::Named::End
                            | iced::keyboard::key::Named::Space
                            | iced::keyboard::key::Named::Enter
                    )
                {
                    return Some(Message::NavigationKey(window, *key, modifiers.shift()));
                }
                if status == iced::event::Status::Ignored
                    && let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                        ..
                    }) = event
                {
                    Some(Message::Stop)
                } else {
                    None
                }
            }),
        ])
    }
    fn send(&self, cmd: Command) {
        if let Some(s) = &self.service {
            s.send(cmd);
        }
    }
    fn pane(&self, p: bool) -> &Pane {
        if p {
            &self.pending_pane
        } else {
            &self.main_pane
        }
    }
    fn pane_mut(&mut self, p: bool) -> &mut Pane {
        if p {
            &mut self.pending_pane
        } else {
            &mut self.main_pane
        }
    }
    fn ids(&self, p: bool) -> Vec<i64> {
        let pane = self.pane(p);
        let visible: BTreeSet<_> = self.filtered(p).into_iter().map(|v| v.id).collect();
        if pane.selected.is_empty() {
            pane.focus
                .into_iter()
                .filter(|id| visible.contains(id))
                .collect()
        } else {
            pane.selected
                .iter()
                .copied()
                .filter(|id| visible.contains(id))
                .collect()
        }
    }
    fn open_pending(&mut self, focus: bool) -> Task<Message> {
        if let Some(id) = self.pending {
            if focus {
                return window::gain_focus(id);
            }
            return Task::none();
        }
        let (id, task) = window::open(window::Settings {
            icon: Some(branding::window_icon()),
            size: iced::Size::new(1180.0, 780.0),
            min_size: Some(iced::Size::new(960.0, 650.0)),
            position: window::Position::Centered,
            exit_on_close_request: false,
            transparent: true,
            ..Default::default()
        });
        self.pending = Some(id);
        task.map(Message::WindowOpened)
    }
    fn stop_player(&mut self) {
        self.playback_jobs
            .stop(self.player.take(), self.loader.take());
        self.playing_file = None;
        self.position = 0.0;
        self.dragging = false;
        self.resume_after_seek = false;
    }
    pub fn update(&mut self, message: Message) -> Task<Message> {
        if self.updates.phase == updates::Phase::Stopping && !matches!(message, Message::Tick) {
            return Task::none();
        }
        if self.updates.installing()
            && matches!(
                message,
                Message::Dismiss
                    | Message::Stop
                    | Message::Import
                    | Message::PickRoots
                    | Message::ShowRoots
                    | Message::NewTag(_)
            )
        {
            return Task::none();
        }
        match message {
            Message::WindowOpened(id) => {
                let dark = self.dark();
                return window::run(id, move |window| {
                    if let Err(error) = branding::set_taskbar_icon(window) {
                        eprintln!("Cannot set the application taskbar icon: {error}");
                    }
                    backdrop::configure(window, dark)
                })
                .map(move |result| Message::BackdropReady(id, dark, result));
            }
            Message::BackdropReady(id, dark, result) => {
                if dark != self.dark() || (id != self.main && Some(id) != self.pending) {
                    return Task::none();
                }
                self.material_windows.remove(&id);
                let note = match result {
                    Ok(()) => {
                        self.material_windows.insert(id);
                        "Windows 11 Desktop Acrylic applied and verified".to_string()
                    }
                    Err(error) => format!("Using opaque backdrop fallback: {error}"),
                };
                use std::io::Write;
                if let Ok(mut log) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.data.join("application.log"))
                {
                    let _ = writeln!(log, "{note}");
                }
            }
            Message::Tick => {
                if self.motion_checked.elapsed() >= Duration::from_secs(1) {
                    motion::refresh_preferences();
                    if self.app_settings.theme == ThemePreference::System
                        && let Some(dark) = settings::system_dark()
                    {
                        self.system_dark = dark;
                    }
                    self.motion_checked = std::time::Instant::now();
                }
                if let Some(task) = self.tick_updates() {
                    return task;
                }
                let events: Vec<_> = self
                    .service
                    .as_ref()
                    .map(|s| s.events.try_iter().collect())
                    .unwrap_or_default();
                let mut open = false;
                let mut tasks = Vec::new();
                for event in events {
                    match event {
                        Event::Snapshot(s) => {
                            self.library_loading = false;
                            self.snapshot = s;
                            if self.reconcile_tags() {
                                tasks.push(listing::scroll_to_top(false));
                            }
                            let visible: BTreeSet<_> =
                                self.snapshot.videos.iter().map(|v| v.id).collect();
                            self.main_pane.selected.retain(|id| visible.contains(id));
                            self.pending_pane.selected.retain(|id| {
                                self.snapshot
                                    .videos
                                    .iter()
                                    .any(|v| v.id == *id && v.pending())
                            });
                            for pending in [false, true] {
                                let total = self.filtered(pending).len();
                                let pane = self.pane_mut(pending);
                                let page = pane.pagination(total).page;
                                if pane.page != page {
                                    pane.set_page(page, total);
                                    tasks.push(listing::scroll_to_top(pending));
                                }
                            }
                        }
                        Event::Pending => open = true,
                        Event::Progress { done, total, bytes } => {
                            self.progress = Some((done, total, bytes))
                        }
                        Event::Status(s) => {
                            self.status = s;
                            self.progress = None;
                        }
                        Event::Error(e) => {
                            self.library_loading = false;
                            self.error = Some(e);
                            self.progress = None;
                        }
                        Event::Settings(s) => self.settings = s,
                        Event::AppSettings(s) => {
                            self.app_settings = s;
                            self.app_settings_loaded = true;
                        }
                        Event::AppSettingsSaved(result) => {
                            self.settings_saving = None;
                            match result {
                                Ok(s) => {
                                    self.app_settings = s;
                                    if self.app_settings.theme == ThemePreference::System
                                        && let Some(dark) = settings::system_dark()
                                    {
                                        self.system_dark = dark;
                                    }
                                }
                                Err(error) => self.error = Some(format!("设置未能保存：{error}")),
                            }
                        }
                        Event::UpdateSettings(result) => {
                            self.updates.settings_received(result, false)
                        }
                        Event::UpdateSettingsSaved(result) => {
                            self.updates.settings_received(result, true)
                        }
                        Event::Stopped => {
                            self.app_settings_loaded = false;
                            self.settings_saving = None;
                            self.library_loading = false;
                            self.status = "后台服务已停止".into();
                            self.progress = None;
                        }
                    }
                }
                self.playback_jobs.reap();
                if let Some((targets, started)) = self.pending_trash.take() {
                    if self.playback_jobs.is_idle() {
                        self.send(Command::Trash(targets));
                    } else if started.elapsed() >= Duration::from_secs(10) {
                        self.error =
                            Some("播放组件尚未释放文件，未移入回收站，请稍后重试。".into());
                    } else {
                        self.pending_trash = Some((targets, started));
                    }
                }
                if self.loader.as_ref().is_some_and(|l| l.expired()) {
                    self.stop_player();
                    self.error = Some("视频加载超时，请检查文件或网络连接后重试。".into());
                }
                if self.loader.as_ref().is_some_and(|l| l.is_finished())
                    && let Some(loader) = self.loader.take()
                {
                    match loader.finish() {
                        Ok(mut video) => {
                            video.set_volume(self.settings.volume);
                            video.set_muted(self.settings.muted);
                            video.set_paused(
                                self.page != MainPage::Library && self.player_window == self.main,
                            );
                            self.player = Some(video);
                        }
                        Err(e) => {
                            self.error = Some(e);
                            self.playing_file = None;
                        }
                    }
                }
                if let Some(error) = self.player.as_ref().and_then(Video::take_error) {
                    self.error = Some(format!("播放出错：{error}"));
                    self.stop_player();
                }
                if let Some(video) = &mut self.player
                    && video.eos()
                    && !video.paused()
                {
                    video.set_paused(true);
                }
                if !self.dragging
                    && let Some(v) = &self.player
                {
                    self.position = v.position().as_secs_f64();
                }
                if open {
                    tasks.push(self.open_pending(false));
                }
                self.sync_covers();
                tasks.push(self.sync_theme());
                return Task::batch(tasks);
            }
            Message::Close(id) => {
                self.material_windows.remove(&id);
                if id == self.main {
                    self.covers.stop();
                    self.stop_player();
                    if let Some(mut s) = self.service.take() {
                        s.shutdown();
                    }
                    return iced::exit();
                }
                if Some(id) == self.pending {
                    self.pending = None;
                    self.pending_pane.tag_draft.clear();
                    if self.player_window == id {
                        self.stop_player();
                        self.fullscreen = false;
                    }
                    if self.dialog.as_ref().is_some_and(|(owner, _)| *owner == id) {
                        self.dialog = None;
                    }
                    return window::close(id);
                }
            }
            Message::OpenPending => return self.open_pending(true),
            Message::Filter(f) => {
                self.tag_filter_open = false;
                self.page = MainPage::Library;
                self.filter = f;
                self.main_pane.selected.clear();
                self.main_pane.tag_draft.clear();
                self.main_pane.focus = None;
                self.main_pane.page = 0;
                self.main_pane.page_input = None;
                return listing::scroll_to_top(false);
            }
            Message::Import => {
                if self.dialog.is_some() {
                    return Task::none();
                }
                return window::run(self.main, |parent| {
                    rfd::AsyncFileDialog::new()
                        .set_parent(parent)
                        .set_title("选择视频文件")
                        .add_filter("视频", VIDEO_EXTENSIONS)
                })
                .then(|dialog| {
                    Task::perform(
                        async move {
                            dialog
                                .pick_files()
                                .await
                                .unwrap_or_default()
                                .into_iter()
                                .map(|f| f.path().to_path_buf())
                                .collect()
                        },
                        Message::Files,
                    )
                });
            }
            Message::PickRoots => {
                return window::run(self.main, |parent| {
                    rfd::AsyncFileDialog::new()
                        .set_parent(parent)
                        .set_title("添加监控文件夹（包含子文件夹）")
                })
                .then(|dialog| {
                    Task::perform(
                        async move {
                            dialog
                                .pick_folders()
                                .await
                                .unwrap_or_default()
                                .into_iter()
                                .map(|f| f.path().to_path_buf())
                                .collect()
                        },
                        Message::Roots,
                    )
                });
            }
            Message::Files(p) => {
                if !p.is_empty() {
                    self.send(Command::Import(p));
                }
            }
            Message::Roots(p) => {
                if !p.is_empty() {
                    self.send(Command::AddRoots(p));
                }
            }
            Message::Scan(f) => self.send(Command::Scan(f)),
            Message::CancelScan => self.send(Command::Cancel),
            Message::Search(p, s) => {
                let pane = self.pane_mut(p);
                pane.search = s;
                pane.page = 0;
                pane.page_input = None;
                pane.selected.clear();
                pane.tag_draft.clear();
                pane.focus = None;
                return listing::scroll_to_top(p);
            }
            Message::Sort(p, s) => {
                let pane = self.pane_mut(p);
                pane.toggle_sort(s);
                return listing::scroll_to_top(p);
            }
            Message::TagFilterOpen(open) => {
                self.tag_filter_open = open;
                if open {
                    self.tag_filter_focus = 0;
                    return widget::operation::focus("tag-filter");
                }
            }
            Message::TagFilterCheck(id, on) => {
                if let Some(index) = self.snapshot.tags.iter().position(|tag| tag.id == id) {
                    self.tag_filter_focus = index;
                    let changed = if on {
                        self.tag_filter.insert(id)
                    } else {
                        self.tag_filter.remove(&id)
                    };
                    if changed {
                        return self.reset_tag_filter_results();
                    }
                }
            }
            Message::ClearTagFilter => {
                if !self.tag_filter.is_empty() {
                    self.tag_filter.clear();
                    return self.reset_tag_filter_results();
                }
            }
            Message::TagFilterKey(key, reverse) => return self.tag_filter_key(key, reverse),
            Message::Select(p, id) => {
                let owner = if p {
                    self.pending.unwrap_or(self.main)
                } else {
                    self.main
                };
                let video = self.snapshot.videos.iter().find(|v| v.id == id);
                // Compare the playing video, not the detail focus: filtering
                // and clearing selection can remove focus while playback stays.
                let switch_playback = (self.player.is_some() || self.loader.is_some())
                    && (self.player_window != owner
                        || !video.is_some_and(|v| {
                            v.files.iter().any(|f| Some(f.id) == self.playing_file)
                        }));
                let playable = video.and_then(|v| {
                    v.files
                        .iter()
                        .find(|f| {
                            f.availability == Availability::Present
                                && f.state
                                    == if p {
                                        FileState::Pending
                                    } else {
                                        FileState::Managed
                                    }
                        })
                        .map(|f| f.id)
                });
                if self.pane(p).selected.is_empty() && self.pane(p).focus != Some(id) {
                    self.pane_mut(p).tag_draft.clear();
                }
                self.pane_mut(p).focus = Some(id);
                self.details_visible[usize::from(p)] = true;
                if switch_playback {
                    self.stop_player();
                    if let Some(file) = playable {
                        return self.update(Message::Play(file, owner));
                    }
                }
            }
            Message::Check(p, id, on) => {
                let pane = self.pane_mut(p);
                pane.tag_draft.clear();
                if on {
                    pane.selected.insert(id);
                } else {
                    pane.selected.remove(&id);
                }
            }
            Message::SelectPage(p) => {
                let filtered = self.filtered(p);
                let range = self.pane(p).pagination(filtered.len()).range();
                let ids: Vec<_> = filtered[range].iter().map(|v| v.id).collect();
                let pane = self.pane_mut(p);
                pane.tag_draft.clear();
                if ids.iter().all(|id| pane.selected.contains(id)) {
                    for id in ids {
                        pane.selected.remove(&id);
                    }
                } else {
                    pane.selected.extend(ids);
                }
            }
            Message::Page(p, page) => {
                let total = self.filtered(p).len();
                self.pane_mut(p).set_page(page, total);
                return listing::scroll_to_top(p);
            }
            Message::PageSize(p, size) => {
                let total = self.filtered(p).len();
                self.pane_mut(p).set_page_size(size, total);
                return listing::scroll_to_top(p);
            }
            Message::PageInput(p, value) => {
                if value.len() <= 10 && value.bytes().all(|c| c.is_ascii_digit()) {
                    self.pane_mut(p).page_input = Some(value);
                }
            }
            Message::JumpPage(p) => {
                let total = self.filtered(p).len();
                if let Some(page) = self.pane(p).jump_target(total) {
                    self.pane_mut(p).set_page(page, total);
                    return listing::scroll_to_top(p);
                }
            }
            Message::PickTags(action) => self.open_tag_picker(action),
            Message::TagCheck(id, on) => {
                if let Some((_, Dialog::Tags(picker))) = &mut self.dialog {
                    picker.check(&self.snapshot, id, on);
                }
            }
            Message::SaveTagSelection => self.save_tag_selection(),
            Message::Assign(p) => {
                let ids = self.ids(p);
                if ids.is_empty() {
                    return Task::none();
                }
                let assignment = self.pending_tag_assignment();
                if p && self.import_changes_shared_tags(&ids, &assignment) {
                    self.dialog = Some((
                        self.pending.unwrap_or(self.main),
                        Dialog::Assign {
                            ids,
                            assignment,
                            accept: true,
                        },
                    ));
                } else {
                    self.send(Command::Assign(ids, assignment, p));
                    self.clear_tag_selection(p);
                }
            }
            Message::ConfirmAssign => {
                if let Some((
                    _,
                    Dialog::Assign {
                        ids,
                        assignment,
                        accept,
                    },
                )) = self.dialog.take()
                {
                    self.send(Command::Assign(ids, assignment, accept));
                    self.clear_tag_selection(accept);
                }
            }
            Message::Forget(p) => {
                self.send(Command::Forget(self.ids(p), p));
                self.clear_tag_selection(p);
            }
            Message::NewTag(id) => {
                self.tag_filter_open = false;
                if id == self.main && self.page == MainPage::Tags {
                    self.tag_management_focus = 1;
                }
                self.tag_name.clear();
                self.dialog = Some((id, Dialog::Tag(None)));
                return widget::operation::focus("tag-name");
            }
            Message::EditTag(id) => {
                self.tag_filter_open = false;
                if let Some(index) = self.snapshot.tags.iter().position(|tag| tag.id == id) {
                    self.tag_management_focus = 2 + index * 2;
                }
                self.tag_name = self
                    .snapshot
                    .tags
                    .iter()
                    .find(|g| g.id == id)
                    .map(|g| g.name.clone())
                    .unwrap_or_default();
                self.dialog = Some((self.main, Dialog::Tag(Some(id))));
                return widget::operation::focus("tag-name");
            }
            Message::DeleteTag(id) => {
                self.tag_filter_open = false;
                if let Some(index) = self.snapshot.tags.iter().position(|tag| tag.id == id) {
                    self.tag_management_focus = 3 + index * 2;
                }
                self.dialog = Some((self.main, Dialog::DeleteTag(id)));
            }
            Message::TagName(name) => self.tag_name = name,
            Message::SaveTag => {
                if self.tag_name.trim().is_empty() || self.tag_name.trim().chars().count() > 80 {
                    self.error = Some("标签名称必须为 1–80 个字符".into());
                    return Task::none();
                }
                if let Some((_, Dialog::Tag(id))) = self.dialog.take() {
                    match id {
                        Some(id) => self.send(Command::RenameTag(id, self.tag_name.clone())),
                        None => self.send(Command::CreateTag(self.tag_name.clone())),
                    }
                }
            }
            Message::ConfirmDeleteTag => {
                if let Some((_, Dialog::DeleteTag(id))) = self.dialog.take() {
                    self.send(Command::DeleteTag(id));
                }
            }
            Message::Trash(p, id) => {
                let ids = self.ids(p);
                let files: Vec<_> = self
                    .snapshot
                    .videos
                    .iter()
                    .filter(|v| ids.contains(&v.id))
                    .flat_map(|v| {
                        v.files
                            .iter()
                            .filter(|f| {
                                if p {
                                    f.state == FileState::Pending
                                } else {
                                    f.state == FileState::Managed
                                }
                            })
                            .cloned()
                    })
                    .collect();
                let mut selected = BTreeSet::new();
                let mut contents = BTreeSet::new();
                for f in &files {
                    if f.availability == Availability::Present && contents.insert(f.video_id) {
                        selected.insert(f.id);
                    }
                }
                let targets = files
                    .iter()
                    .filter_map(|f| {
                        self.snapshot
                            .videos
                            .iter()
                            .find(|v| v.id == f.video_id)
                            .map(|v| DeleteTarget {
                                file_id: f.id,
                                path: f.path.clone(),
                                hash: v.hash.clone(),
                            })
                    })
                    .collect();
                self.dialog = Some((
                    id,
                    Dialog::Trash {
                        files,
                        selected,
                        targets,
                    },
                ));
            }
            Message::TrashCheck(id, on) => {
                if let Some((_, Dialog::Trash { selected, .. })) = &mut self.dialog {
                    if on {
                        selected.insert(id);
                    } else {
                        selected.remove(&id);
                    }
                }
            }
            Message::ConfirmTrash => {
                if self.pending_trash.is_some() {
                    self.error = Some("正在等待播放器释放文件，请稍后重试。".into());
                    return Task::none();
                }
                if let Some((
                    _,
                    Dialog::Trash {
                        selected, targets, ..
                    },
                )) = self.dialog.take()
                {
                    if self.playing_file.is_some_and(|id| selected.contains(&id)) {
                        self.stop_player();
                    }
                    let targets: Vec<_> = targets
                        .into_iter()
                        .filter(|t| selected.contains(&t.file_id))
                        .collect();
                    // Asynchronous stop may still own file handles. Preserve
                    // the confirmed targets, but never wait in the event loop.
                    if self.playback_jobs.is_idle() {
                        self.send(Command::Trash(targets));
                    } else {
                        self.pending_trash = Some((targets, std::time::Instant::now()));
                    }
                }
            }
            Message::Dismiss => {
                if matches!(
                    self.dialog,
                    Some((_, Dialog::UpdateAvailable | Dialog::Updates))
                ) {
                    self.updates.dismiss();
                }
                self.dialog = None;
            }
            Message::ShowUpdates => self.dialog = Some((self.main, Dialog::Updates)),
            Message::SettingFocus(index) => self.setting_focus = index,
            Message::NavigationKey(id, key, reverse) => {
                if matches!(&self.dialog, Some((owner, Dialog::Tags(_))) if *owner == id) {
                    return self.tag_picker_key(key, reverse);
                }
                if id == self.main && self.page == MainPage::Tags && self.dialog.is_none() {
                    return self.tag_management_key(key, reverse);
                }
                if id == self.main && self.page == MainPage::Settings && self.dialog.is_none() {
                    self.settings_key(key);
                } else if self.dialog.is_none() && key == iced::keyboard::key::Named::Tab {
                    return if reverse {
                        widget::operation::focus_previous()
                    } else {
                        widget::operation::focus_next()
                    };
                }
            }
            Message::ShowSettings => {
                self.show_page(MainPage::Settings);
            }
            Message::ShowTagManagement => {
                self.show_page(MainPage::Tags);
                self.tag_management_focus = usize::from(self.tag_management_enabled());
                return Task::batch([
                    window::gain_focus(self.main),
                    iced::advanced::widget::operate(
                        iced::advanced::widget::operation::focusable::unfocus(),
                    ),
                ]);
            }
            Message::ShowLibrary => self.show_page(MainPage::Library),
            Message::Concurrency(concurrency) => self.save_app_settings(AppSettings {
                concurrency,
                ..self.app_settings.clone()
            }),
            Message::Theme(theme) => self.save_app_settings(AppSettings {
                theme,
                ..self.app_settings.clone()
            }),
            Message::PickUpdateFolder => {
                if !self.updates.can_configure() {
                    return Task::none();
                }
                return window::run(self.main, |parent| {
                    rfd::AsyncFileDialog::new()
                        .set_parent(parent)
                        .set_title("选择更新文件夹（本机磁盘或移动硬盘）")
                })
                .then(|dialog| {
                    Task::perform(
                        async move { dialog.pick_folder().await.map(|f| f.path().to_path_buf()) },
                        Message::UpdateFolder,
                    )
                });
            }
            Message::UpdateFolder(path) => {
                if let Some(path) = path {
                    self.updates.folder(path);
                }
            }
            Message::ClearUpdateFolder => self.save_update_folder(None),
            Message::CheckUpdate => {
                if self.updates.phase == updates::Phase::Idle && self.updates.can_configure() {
                    self.updates.check(true);
                }
            }
            Message::InstallUpdate => self.install_update(),
            Message::ShowRoots => self.dialog = Some((self.main, Dialog::Roots)),
            Message::RemoveRoot(p) => self.send(Command::RemoveRoot(p)),
            Message::Play(id, owner) => {
                if self
                    .pending_trash
                    .as_ref()
                    .is_some_and(|(targets, _)| targets.iter().any(|t| t.file_id == id))
                {
                    self.error = Some("此文件正在移入回收站，请稍候。".into());
                    return Task::none();
                }
                let info = self.snapshot.videos.iter().find_map(|v| {
                    v.files
                        .iter()
                        .find(|f| f.id == id)
                        .map(|f| (f.path.clone(), v.title.clone(), v.id))
                });
                if let Some((path, title, video_id)) = info {
                    let previous_fullscreen = (self.fullscreen && self.player_window != owner)
                        .then_some(self.player_window);
                    if previous_fullscreen.is_some() {
                        self.fullscreen = false;
                    }
                    self.stop_player();
                    if !self.playback_jobs.can_start() {
                        self.error = Some(
                            "播放组件仍在释放资源，请稍后重试；若持续无法恢复，请重启程序。".into(),
                        );
                        if let Some(previous) = previous_fullscreen {
                            return window::set_mode(previous, window::Mode::Windowed);
                        }
                        return Task::none();
                    }
                    self.pane_mut(Some(owner) == self.pending).focus = Some(video_id);
                    self.details_visible[usize::from(Some(owner) == self.pending)] = true;
                    self.playing_file = Some(id);
                    self.player_title = title;
                    self.player_window = owner;
                    match playback::Load::start(path) {
                        Ok(load) => self.loader = Some(load),
                        Err(error) => {
                            self.playing_file = None;
                            self.error = Some(format!("无法启动播放组件：{error}"));
                        }
                    }
                    if let Some(previous) = previous_fullscreen {
                        return window::set_mode(previous, window::Mode::Windowed);
                    }
                }
            }
            Message::Pause => {
                if let Some(v) = &mut self.player {
                    v.set_paused(!v.paused());
                }
            }
            Message::Seek(pos) => {
                if let Some(v) = &mut self.player {
                    if !self.dragging {
                        self.resume_after_seek = !v.paused();
                        v.set_paused(true);
                    }
                    self.dragging = true;
                    self.position = pos;
                }
            }
            Message::SeekRelease => {
                if let Some(v) = &mut self.player {
                    if let Err(e) = v.seek(Duration::from_secs_f64(self.position.max(0.0)), false) {
                        self.error = Some(format!("跳转失败：{e}"));
                    }
                    v.set_paused(!self.resume_after_seek);
                }
                self.dragging = false;
            }
            Message::Volume(volume) => {
                self.settings.volume = volume;
                if let Some(v) = &mut self.player {
                    v.set_volume(volume);
                }
                self.send(Command::Settings(self.settings.clone()));
            }
            Message::Mute => {
                self.settings.muted = !self.settings.muted;
                if let Some(v) = &mut self.player {
                    v.set_muted(self.settings.muted);
                }
                self.send(Command::Settings(self.settings.clone()));
            }
            Message::Fullscreen(id) => {
                self.fullscreen = !self.fullscreen;
                return window::set_mode(
                    id,
                    if self.fullscreen {
                        window::Mode::Fullscreen
                    } else {
                        window::Mode::Windowed
                    },
                );
            }
            Message::Stop => {
                if self.dialog.is_some() {
                    if matches!(
                        self.dialog,
                        Some((_, Dialog::UpdateAvailable | Dialog::Updates))
                    ) {
                        self.updates.dismiss();
                    }
                    self.dialog = None;
                    return Task::none();
                }
                if self.fullscreen {
                    self.fullscreen = false;
                    return window::set_mode(self.player_window, window::Mode::Windowed);
                }
                self.stop_player();
            }
            Message::Frame => {}
            Message::Copy(v) => return iced::clipboard::write(v),
            Message::OpenInExplorer(path) => self.send(Command::OpenInExplorer(path)),
            Message::ClearError => self.error = None,
            Message::ClearUpdateNotice => self.update_notice = None,
            Message::ToggleDetails(p) => {
                self.details_visible[usize::from(p)] = !self.details_visible[usize::from(p)];
                let owner = if p { self.pending } else { Some(self.main) };
                if !self.details_visible[usize::from(p)] && owner == Some(self.player_window) {
                    self.stop_player();
                }
            }
            Message::ResizeDetails(p, width) => {
                if width.is_finite() {
                    self.details_width[usize::from(p)] =
                        width.clamp(DETAILS_MIN_WIDTH, DETAILS_MAX_WIDTH);
                }
            }
            Message::ClearSelection(p) => {
                self.clear_tag_selection(p);
            }
            Message::FocusSearch(id) => {
                if self.dialog.is_none() {
                    if id == self.main {
                        self.tag_filter_open = false;
                        self.page = MainPage::Library;
                    }
                    return widget::operation::focus(if Some(id) == self.pending {
                        "pending-search"
                    } else {
                        "library-search"
                    });
                }
            }
        }
        Task::none()
    }
    fn filtered(&self, p: bool) -> Vec<&VideoItem> {
        let pane = self.pane(p);
        let query = pane.search.to_lowercase();
        let mut list: Vec<_> = self
            .snapshot
            .videos
            .iter()
            .filter(|v| {
                let in_view = if p {
                    v.pending()
                } else {
                    v.library
                        && match self.filter {
                            Filter::All => true,
                            Filter::Untagged => v.tag_ids.is_empty(),
                            Filter::Duplicates => {
                                v.managed_files()
                                    .filter(|f| f.availability == Availability::Present)
                                    .count()
                                    > 1
                            }
                            Filter::Missing => v.missing(),
                        }
                        && self.tag_filter.iter().all(|id| v.tag_ids.contains(id))
                };
                in_view
                    && (query.is_empty()
                        || v.title.to_lowercase().contains(&query)
                        || v.files
                            .iter()
                            .any(|f| f.path.to_string_lossy().to_lowercase().contains(&query)))
            })
            .collect();
        pane.sort.sort(&mut list, pane.sort_order, p);
        list
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.covers.stop();
        self.stop_player();
        if let Some(s) = &mut self.service {
            s.shutdown();
        }
    }
}
fn time_label(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}
