use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::OnceLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileState {
    Pending,
    Managed,
    Ignored,
}
impl FileState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Managed => "managed",
            Self::Ignored => "ignored",
        }
    }
    pub fn parse(value: &str) -> Self {
        match value {
            "managed" => Self::Managed,
            "ignored" => Self::Ignored,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Present,
    Missing,
    Inaccessible,
}
impl Availability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Inaccessible => "inaccessible",
        }
    }
    pub fn parse(value: &str) -> Self {
        match value {
            "missing" => Self::Missing,
            "inaccessible" => Self::Inaccessible,
            _ => Self::Present,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Present => "可访问",
            Self::Missing => "文件缺失",
            Self::Inaccessible => "不可访问",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VideoFile {
    pub id: i64,
    pub video_id: i64,
    pub path: PathBuf,
    pub size: u64,
    pub modified: i64,
    pub state: FileState,
    pub availability: Availability,
}

#[derive(Debug, Clone)]
pub struct VideoItem {
    pub id: i64,
    pub hash: String,
    pub title: String,
    pub tag_ids: Vec<i64>,
    pub library: bool,
    pub created_at: i64,
    pub files: Vec<VideoFile>,
}
impl VideoItem {
    pub fn pending(&self) -> bool {
        self.files.iter().any(|f| f.state == FileState::Pending)
    }
    pub fn managed_files(&self) -> impl Iterator<Item = &VideoFile> {
        self.files.iter().filter(|f| f.state == FileState::Managed)
    }
    pub fn missing(&self) -> bool {
        self.library
            && !self
                .managed_files()
                .any(|f| f.availability == Availability::Present)
    }
    pub fn size(&self) -> u64 {
        self.files.first().map(|f| f.size).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub videos: Vec<VideoItem>,
    pub tags: Vec<Tag>,
    pub roots: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Fingerprint {
    pub path: PathBuf,
    pub size: u64,
    pub modified: i64,
    pub hash: String,
    pub explicit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagAssignment {
    Keep,
    Add(Vec<i64>),
    Remove(Vec<i64>),
}

#[derive(Debug, Clone)]
pub struct DeleteTarget {
    pub file_id: i64,
    pub path: PathBuf,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub volume: f64,
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemePreference {
    #[default]
    Light,
    Dark,
    System,
}
impl std::fmt::Display for ThemePreference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Light => "浅色",
            Self::Dark => "深色",
            Self::System => "跟随系统",
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub concurrency: usize,
    pub theme: ThemePreference,
}
impl AppSettings {
    /// Keep the UI, persisted settings, and worker pool on one limit per launch.
    pub fn max_concurrency() -> usize {
        static MAX: OnceLock<usize> = OnceLock::new();
        *MAX.get_or_init(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2)
        })
    }
}
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            concurrency: 2.min(Self::max_concurrency()),
            theme: ThemePreference::Light,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSettings {
    pub folder: Option<PathBuf>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 0.7,
            muted: false,
        }
    }
}

pub fn bytes_label(bytes: u64) -> String {
    let mut n = bytes as f64;
    for suffix in ["B", "KB", "MB", "GB", "TB"] {
        if n < 1024.0 || suffix == "TB" {
            return format!("{n:.1} {suffix}");
        }
        n /= 1024.0;
    }
    unreachable!()
}
