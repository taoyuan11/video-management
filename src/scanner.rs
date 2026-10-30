use crate::processing::{Executor, Lane};
use crate::{database::path_key, model::*};
use anyhow::{Context, Result, bail};
use crossbeam_channel::Sender;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, Metadata, OpenOptions},
    io::Read,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts", "mts", "m2ts",
    "3gp",
];

pub fn is_video(path: &Path) -> bool {
    path.extension().is_some_and(|e| {
        VIDEO_EXTENSIONS.contains(&e.to_string_lossy().to_ascii_lowercase().as_str())
    })
}
pub fn normalize(path: &Path) -> Result<PathBuf> {
    dunce::canonicalize(path).with_context(|| format!("无法访问 {}", path.display()))
}
pub fn modified(meta: &Metadata) -> i64 {
    meta.modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64
}

#[derive(Default)]
pub struct ScanStats {
    pub total: AtomicUsize,
    pub done: AtomicUsize,
    pub bytes: AtomicU64,
    pub deferred: AtomicUsize,
}
pub struct ScanRequest {
    pub roots: Vec<PathBuf>,
    pub known: Vec<VideoFile>,
    pub explicit: Vec<PathBuf>,
    pub full: bool,
    pub durations: BTreeMap<i64, DurationStatus>,
}
pub enum ScanEvent {
    Availability(PathBuf, Availability),
    Found(Fingerprint),
    Duration(String, Option<u64>),
    Warning(String),
    Retry(PathBuf),
    Finished,
}

/// A deny-write handle on Windows prevents hashing a file while another process is writing it.
pub fn fingerprint(
    path: &Path,
    explicit: bool,
    cancel: &AtomicBool,
    stats: &ScanStats,
) -> Result<Fingerprint> {
    let mut options = OpenOptions::new();
    options.read(true);
    options.share_mode(1); // FILE_SHARE_READ; do not allow writes or replacement during hashing.
    let mut file = options.open(path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        bail!("不是普通文件");
    }
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("已取消");
        }
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        stats.bytes.fetch_add(n as u64, Ordering::Relaxed);
    }
    let after = file.metadata()?;
    let current = fs::metadata(path)?;
    if before.len() != after.len()
        || modified(&before) != modified(&after)
        || after.len() != current.len()
        || modified(&after) != modified(&current)
    {
        bail!("文件仍在变化，稍后重试");
    }
    Ok(Fingerprint {
        path: path.to_path_buf(),
        size: after.len(),
        modified: modified(&after),
        hash: hash.finalize().iter().map(|b| format!("{b:02x}")).collect(),
        explicit,
    })
}

pub fn scan(
    request: ScanRequest,
    sender: Sender<ScanEvent>,
    cancel: Arc<AtomicBool>,
    stats: Arc<ScanStats>,
    executor: Executor,
) {
    let mut paths = BTreeMap::<String, (PathBuf, bool)>::new();
    let root_available: Vec<_> = request
        .roots
        .iter()
        .map(|root| (root.clone(), fs::read_dir(root).is_ok()))
        .collect();
    for file in &request.known {
        if cancel.load(Ordering::Relaxed) {
            let _ = sender.send(ScanEvent::Finished);
            return;
        }
        let status = match fs::metadata(&file.path) {
            Ok(m) if m.is_file() => {
                paths.insert(path_key(&file.path), (file.path.clone(), false));
                Availability::Present
            }
            Ok(_) => Availability::Missing,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if root_available
                    .iter()
                    .any(|(r, ok)| file.path.starts_with(r) && !*ok)
                {
                    Availability::Inaccessible
                } else {
                    Availability::Missing
                }
            }
            Err(_) => Availability::Inaccessible,
        };
        let _ = sender.send(ScanEvent::Availability(file.path.clone(), status));
    }
    for root in &request.roots {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_link(e.path()))
        {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            match entry {
                Ok(entry) if entry.file_type().is_file() && is_video(entry.path()) => {
                    let path = entry.path().to_path_buf();
                    paths.entry(path_key(&path)).or_insert((path, false));
                }
                Err(e) => {
                    let _ = sender.send(ScanEvent::Warning(format!("扫描跳过：{e}")));
                }
                _ => {}
            }
        }
    }
    for path in request.explicit {
        if is_video(&path) {
            paths.insert(path_key(&path), (path, true));
        } else {
            let _ = sender.send(ScanEvent::Warning(format!(
                "不支持的文件扩展名：{}",
                path.display()
            )));
        }
    }
    let known: BTreeMap<_, _> = request
        .known
        .iter()
        .map(|f| (path_key(&f.path), f))
        .collect();
    let cached_durations = Arc::new(
        request
            .durations
            .values()
            .filter(|status| status.checked)
            .map(|status| status.hash.clone())
            .collect::<HashSet<_>>(),
    );
    let jobs: Vec<_> = paths
        .into_iter()
        .filter_map(|(key, (path, explicit))| {
            if !request.full
                && !explicit
                && let Some(old) = known.get(&key)
                && let Ok(meta) = fs::metadata(&path)
                && old.size == meta.len()
                && old.modified == modified(&meta)
            {
                // Backfill existing libraries without rehashing unchanged files.
                return request
                    .durations
                    .get(&old.video_id)
                    .filter(|status| {
                        cfg!(feature = "gui") && !status.checked && old.state != FileState::Ignored
                    })
                    .map(|status| {
                        (
                            path,
                            explicit,
                            Some(Fingerprint {
                                path: old.path.clone(),
                                size: old.size,
                                modified: old.modified,
                                hash: status.hash.clone(),
                                explicit,
                            }),
                        )
                    });
            }
            Some((path, explicit, None))
        })
        .collect();
    stats.total.store(jobs.len(), Ordering::Relaxed);
    let (complete, completed) = crossbeam_channel::unbounded::<()>();
    for (path, explicit, metadata_only) in jobs {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let (cancel, stats, sender, done) = (
            cancel.clone(),
            stats.clone(),
            sender.clone(),
            complete.clone(),
        );
        let cached_durations = cached_durations.clone();
        executor.submit(Lane::Scan, cancel.clone(), move || {
            // Dropping this sender also completes cancelled queued jobs and panics.
            let _done = done;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            // Settle new/changed files; open sharing rules and the post-hash stat are the final guard.
            if let Ok(meta) = fs::metadata(&path)
                && let Ok(stamp) = meta.modified()
            {
                let settle = Duration::from_secs(2)
                    .saturating_sub(SystemTime::now().duration_since(stamp).unwrap_or_default());
                let start = Instant::now();
                while start.elapsed() < settle {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            let metadata_only_job = metadata_only.is_some();
            match metadata_only
                .map(Ok)
                .unwrap_or_else(|| fingerprint(&path, explicit, &cancel, &stats))
            {
                Ok(found) => {
                    if !metadata_only_job {
                        let _ = sender.send(ScanEvent::Found(found.clone()));
                    }
                    if !cached_durations.contains(&found.hash) {
                        read_duration(&found, &cancel, &sender);
                    }
                }
                Err(e) => {
                    stats.deferred.fetch_add(1, Ordering::Relaxed);
                    if !cancel.load(Ordering::Relaxed) {
                        if explicit {
                            let _ = sender.send(ScanEvent::Retry(path.clone()));
                        }
                        let _ = sender.send(ScanEvent::Warning(format!(
                            "稍后重试 {}：{e}",
                            path.display()
                        )));
                    }
                }
            }
            stats.done.fetch_add(1, Ordering::Relaxed);
        });
    }
    drop(complete);
    while completed.recv_timeout(Duration::from_millis(50))
        != Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    {
        if cancel.load(Ordering::Relaxed) {
            executor.discard_cancelled();
        }
    }
    let _ = sender.send(ScanEvent::Finished);
}

#[cfg(feature = "gui")]
fn read_duration(found: &Fingerprint, cancel: &AtomicBool, sender: &Sender<ScanEvent>) {
    // Keep the same deny-write guard as hashing while reading container metadata.
    // A missing, locked or replaced source remains eligible for a later scan.
    let Ok(file) = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&found.path)
    else {
        return;
    };
    let matches = |meta: Metadata| meta.len() == found.size && modified(&meta) == found.modified;
    if !file.metadata().is_ok_and(matches) {
        return;
    }
    let duration = crate::media::duration_ms(&found.path, cancel)
        .ok()
        .flatten();
    if !cancel.load(Ordering::Relaxed) && fs::metadata(&found.path).is_ok_and(matches) {
        let _ = sender.send(ScanEvent::Duration(found.hash.clone(), duration));
    }
}

#[cfg(not(feature = "gui"))]
fn read_duration(_found: &Fingerprint, _cancel: &AtomicBool, _sender: &Sender<ScanEvent>) {}

fn is_link(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    meta.file_attributes() & 0x400 != 0 // includes junctions and directory reparse points
}
