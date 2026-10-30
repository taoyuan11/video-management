use super::*;
use std::ops::Range;
use windows_sys::Win32::{
    Foundation::{FILETIME, SYSTEMTIME},
    System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTimeEx},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Sort {
    #[default]
    Recent,
    Name,
    Size,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum SortOrder {
    Ascending,
    #[default]
    Descending,
}

impl SortOrder {
    pub fn reverse(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "最近导入",
            Self::Name => "名称",
            Self::Size => "大小",
            Self::Modified => "修改时间",
        }
    }

    fn default_order(self) -> SortOrder {
        if self == Self::Name {
            SortOrder::Ascending
        } else {
            SortOrder::Descending
        }
    }

    pub fn sort(self, videos: &mut [&VideoItem], order: SortOrder, pending: bool) {
        // The ID tie-breaker keeps page membership stable when primary keys match.
        match self {
            Self::Recent => videos.sort_by_key(|v| (v.created_at, v.id)),
            Self::Name => videos.sort_by_cached_key(|v| (v.title.to_lowercase(), v.id)),
            Self::Size => videos.sort_by_key(|v| (v.size(), v.id)),
            Self::Modified => videos.sort_by_key(|v| (modified(v, pending), v.id)),
        }
        if order == SortOrder::Descending {
            videos.reverse();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum PageSize {
    Twenty,
    Forty,
    #[default]
    Sixty,
}

impl PageSize {
    pub const ALL: [Self; 3] = [Self::Twenty, Self::Forty, Self::Sixty];

    pub fn count(self) -> usize {
        match self {
            Self::Twenty => 20,
            Self::Forty => 40,
            Self::Sixty => 60,
        }
    }
}

impl std::fmt::Display for PageSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} 条 / 页", self.count())
    }
}

pub struct Pagination {
    pub page: usize,
    pub pages: usize,
    pub start: usize,
    pub end: usize,
}

impl Pagination {
    fn new(page: usize, page_size: PageSize, total: usize) -> Self {
        let size = page_size.count();
        let pages = total.div_ceil(size).max(1);
        let page = page.min(pages - 1);
        let start = page * size;
        Self {
            page,
            pages,
            start,
            end: start.saturating_add(size).min(total),
        }
    }

    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }
}

impl Pane {
    pub fn pagination(&self, total: usize) -> Pagination {
        Pagination::new(self.page, self.page_size, total)
    }

    pub fn set_page(&mut self, page: usize, total: usize) {
        self.page = Pagination::new(page, self.page_size, total).page;
        self.page_input = None;
    }

    pub fn set_page_size(&mut self, size: PageSize, total: usize) {
        // Keep the old page's first item in view when changing density.
        let first = self.pagination(total).start;
        self.page_size = size;
        self.set_page(first / size.count(), total);
    }

    pub fn jump_target(&self, total: usize) -> Option<usize> {
        let target = self.page_input.as_ref()?.parse::<usize>().ok()?;
        (total > 0 && (1..=self.pagination(total).pages).contains(&target)).then(|| target - 1)
    }

    pub fn toggle_sort(&mut self, sort: Sort) {
        self.sort_order = if self.sort == sort {
            self.sort_order.reverse()
        } else {
            sort.default_order()
        };
        self.sort = sort;
        self.page = 0;
        self.page_input = None;
    }
}

pub fn scroll_id(pending: bool) -> &'static str {
    if pending {
        "pending-list"
    } else {
        "library-list"
    }
}

pub fn scroll_to_top(pending: bool) -> Task<Message> {
    widget::operation::snap_to(scroll_id(pending), scrollable::RelativeOffset::START)
}

pub fn modified(video: &VideoItem, pending: bool) -> Option<i64> {
    let state = if pending {
        FileState::Pending
    } else {
        FileState::Managed
    };
    // Copies can have different timestamps. Use the newest relevant copy, including
    // its last known value when offline; ignored copies must not affect the list.
    video
        .files
        .iter()
        .filter(|file| file.state == state && file.modified > 0)
        .map(|file| file.modified)
        .max()
}

fn utc_time(nanos: i64) -> Option<SYSTEMTIME> {
    if nanos <= 0 {
        return None;
    }
    // Stored values are Unix nanoseconds; FILETIME uses 100 ns ticks since 1601.
    let ticks = nanos as u64 / 100 + 116_444_736_000_000_000;
    let file = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    // Both pointers refer to correctly sized structures for the duration of the call.
    (unsafe { FileTimeToSystemTime(&file, &mut utc) } != 0).then_some(utc)
}

pub fn modified_label(nanos: Option<i64>) -> Option<(String, String)> {
    let utc = utc_time(nanos?)?;
    let mut local = SYSTEMTIME::default();
    // A null zone selects Windows' current zone, with the rules for this date.
    if unsafe { SystemTimeToTzSpecificLocalTimeEx(std::ptr::null(), &utc, &mut local) } == 0 {
        return None;
    }
    Some((
        format!("{:04}-{:02}-{:02}", local.wYear, local.wMonth, local.wDay),
        format!(
            "{:02}:{:02}:{:02}",
            local.wHour, local.wMinute, local.wSecond
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: i64, title: &str, size: u64, stamp: i64) -> VideoItem {
        VideoItem {
            id,
            hash: format!("{id:064x}"),
            title: title.into(),
            tag_ids: Vec::new(),
            library: true,
            created_at: id,
            files: vec![VideoFile {
                id,
                video_id: id,
                path: format!("{title}.mp4").into(),
                size,
                modified: stamp,
                state: FileState::Managed,
                availability: Availability::Present,
            }],
        }
    }

    #[test]
    fn sorting_uses_actual_values_in_both_directions_with_stable_ties() {
        let videos = [
            video(3, "alpha", 90, 500),
            video(2, "Beta", 100, 20),
            video(1, "ALPHA", 90, 500),
        ];
        for (sort, expected) in [
            (Sort::Name, [1, 3, 2]),
            (Sort::Size, [1, 3, 2]),
            (Sort::Modified, [2, 1, 3]),
            (Sort::Recent, [1, 2, 3]),
        ] {
            let mut list: Vec<_> = videos.iter().collect();
            sort.sort(&mut list, SortOrder::Ascending, false);
            assert_eq!(list.iter().map(|v| v.id).collect::<Vec<_>>(), expected);
            list.rotate_left(1);
            sort.sort(&mut list, SortOrder::Descending, false);
            assert_eq!(
                list.iter().map(|v| v.id).collect::<Vec<_>>(),
                expected.into_iter().rev().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn modified_uses_only_relevant_copies_and_preserves_offline_values() {
        let mut item = video(1, "视频", 100, 10);
        for (stamp, state) in [
            (30, FileState::Managed),
            (50, FileState::Pending),
            (90, FileState::Ignored),
        ] {
            item.files.push(VideoFile {
                modified: stamp,
                state,
                availability: Availability::Missing,
                ..item.files[0].clone()
            });
        }
        assert_eq!(modified(&item, false), Some(30));
        assert_eq!(modified(&item, true), Some(50));
        item.files.retain(|f| f.state == FileState::Ignored);
        assert_eq!(modified(&item, false), None);
        assert_eq!(modified(&item, true), None);
        item.files[0].state = FileState::Managed;
        item.files[0].modified = 0;
        assert_eq!(modified(&item, false), None);
    }

    #[test]
    fn windows_timestamp_conversion_respects_nanosecond_units_and_unknown_values() {
        // 2024-02-29 12:34:56.789 UTC, independent of the machine's time zone.
        let stamp = 1_709_210_096_789_000_000;
        let utc = utc_time(stamp).unwrap();
        assert_eq!((utc.wYear, utc.wMonth, utc.wDay), (2024, 2, 29));
        assert_eq!(
            (utc.wHour, utc.wMinute, utc.wSecond, utc.wMilliseconds),
            (12, 34, 56, 789)
        );
        let (date, time) = modified_label(Some(stamp)).unwrap();
        assert_eq!(date.len(), 10);
        assert_eq!(time.len(), 8);
        assert!(modified_label(Some(i64::MAX)).is_some());
        for value in [None, Some(0), Some(-1)] {
            assert_eq!(modified_label(value), None);
        }
    }

    #[test]
    fn page_ranges_cover_empty_exact_and_partial_pages_without_overflow() {
        for size in PageSize::ALL {
            for total in [0, 1, size.count(), size.count() + 1, 243, usize::MAX] {
                for requested in [0, 1, 99, usize::MAX] {
                    let page = Pagination::new(requested, size, total);
                    assert!(page.page < page.pages);
                    assert!(page.start <= page.end && page.end <= total);
                    assert!(page.range().len() <= size.count());
                    if total == 0 {
                        assert_eq!(page.range(), 0..0);
                    }
                    if requested == usize::MAX {
                        assert_eq!(page.end, total);
                    }
                }
            }
        }
        assert_eq!(Pagination::new(2, PageSize::Sixty, 121).range(), 120..121);
        assert_eq!(Pagination::new(2, PageSize::Sixty, 120).range(), 60..120);
    }

    #[test]
    fn density_changes_keep_the_anchor_and_clamping_updates_real_state() {
        let mut pane = Pane::default();
        pane.set_page(2, 135);
        pane.set_page_size(PageSize::Twenty, 135);
        assert_eq!(pane.pagination(135).range(), 120..135);
        pane.set_page_size(PageSize::Forty, 135);
        assert_eq!(pane.pagination(135).range(), 120..135);
        pane.page_input = Some("4".into());
        pane.set_page(pane.page, 40);
        assert_eq!(pane.page, 0);
        assert_eq!(pane.page_input, None);
        assert_eq!(pane.pagination(135).page, 0);
    }

    #[test]
    fn page_jump_rejects_invalid_input_and_uses_one_based_numbers() {
        let mut pane = Pane::default();
        for invalid in ["", "0", "4", "-1", "x", "9999999999999999999999999"] {
            pane.page_input = Some(invalid.into());
            assert_eq!(pane.jump_target(121), None);
        }
        for (value, expected) in [("1", 0), ("2", 1), ("03", 2)] {
            pane.page_input = Some(value.into());
            assert_eq!(pane.jump_target(121), Some(expected));
            assert_eq!(pane.jump_target(0), None);
        }
    }

    #[test]
    fn header_switch_resets_page_and_uses_expected_direction() {
        let mut pane = Pane::default();
        pane.set_page(2, 121);
        pane.page_input = Some("3".into());
        pane.selected.insert(7);
        for (sort, order) in [
            (Sort::Name, SortOrder::Ascending),
            (Sort::Name, SortOrder::Descending),
            (Sort::Size, SortOrder::Descending),
            (Sort::Size, SortOrder::Ascending),
            (Sort::Modified, SortOrder::Descending),
            (Sort::Modified, SortOrder::Ascending),
        ] {
            pane.toggle_sort(sort);
            assert_eq!(pane.sort_order, order);
            assert_eq!(pane.page, 0);
            assert_eq!(pane.page_input, None);
            assert!(pane.selected.contains(&7));
        }
    }

    #[test]
    fn window_pagination_and_page_selection_stay_independent() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(dir.path().to_path_buf(), None, None);
        // No event loop or windows are opened. Stop the temporary service before
        // replacing its data with deterministic, entirely in-memory fixtures.
        drop(app.service.take());
        app.snapshot.videos = (1..=65)
            .map(|id| {
                let mut item = video(id, &format!("视频{id:03}"), id as u64, id);
                item.files.push(VideoFile {
                    state: FileState::Pending,
                    ..item.files[0].clone()
                });
                item
            })
            .collect();
        let _ = app.update(Message::Sort(false, Sort::Name));
        let _ = app.update(Message::PageSize(false, PageSize::Twenty));
        let _ = app.update(Message::SelectPage(false));
        assert_eq!(app.main_pane.selected, (1..=20).collect());
        let _ = app.update(Message::Page(false, 3));
        let _ = app.update(Message::SelectPage(false));
        assert_eq!(app.main_pane.selected, (1..=20).chain(61..=65).collect());
        let _ = app.update(Message::SelectPage(false));
        assert_eq!(app.main_pane.selected, (1..=20).collect());
        assert_eq!(app.pending_pane.page_size, PageSize::Sixty);
        assert!(app.pending_pane.selected.is_empty());
        let _ = app.update(Message::PageInput(true, "2".into()));
        let _ = app.update(Message::JumpPage(true));
        let _ = app.update(Message::SelectPage(true));
        assert_eq!(app.pending_pane.selected, (1..=5).collect());
        assert_eq!(app.main_pane.page, 3);
        let _ = app.update(Message::Search(false, "视频061".into()));
        assert_eq!(app.main_pane.page, 0);
        assert!(app.main_pane.selected.is_empty());
        let _ = app.update(Message::SelectPage(false));
        assert_eq!(app.main_pane.selected, BTreeSet::from([61]));
        assert_eq!(app.pending_pane.page, 1);
    }
}
