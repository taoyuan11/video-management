//! Recoverable moves of complete monitored directory trees. Only the service owns SQLite.
use crate::{database::Database, scanner};
use anyhow::{Context, Result, bail, ensure};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::windows::io::AsRawHandle,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, COPY_FILE_FAIL_IF_EXISTS, CopyFileExW, FILE_DISPOSITION_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FileDispositionInfo, FindClose,
    FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard, GetFileInformationByHandle,
    MOVEFILE_WRITE_THROUGH, MoveFileExW, PROGRESS_CANCEL, PROGRESS_CONTINUE,
    SetFileInformationByHandle, WIN32_FIND_STREAM_DATA,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    volume: u32,
    index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    relative: PathBuf,
    directory: bool,
    source_id: Identity,
    destination_id: Option<Identity>,
    size: u64,
    bytes: u64,
    modified: i64,
    hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    pub source: PathBuf,
    pub destination: PathBuf,
    staging: Option<PathBuf>,
    source_id: Identity,
    staging_id: Option<Identity>,
    entries: Vec<Entry>,
    pub committed: bool,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub stage: String,
    pub path: Option<PathBuf>,
    pub done: u64,
    pub total: u64,
    pub cancellable: bool,
}
impl Progress {
    pub fn preparing() -> Self {
        Self {
            stage: "正在准备迁移…".into(),
            path: None,
            done: 0,
            total: 0,
            cancellable: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub message: String,
    pub retry: bool,
}

pub enum Request {
    Save(Journal, Sender<Result<(), String>>),
    Commit(Journal, Sender<Result<(), String>>),
    Load(Sender<Result<Option<Journal>, String>>),
    Clear(Sender<Result<(), String>>),
    Progress(Progress),
    Finished(Outcome),
}

pub struct Worker {
    pub events: Receiver<Request>,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn start(
        source: PathBuf,
        parent: PathBuf,
        protected: Vec<PathBuf>,
        force_copy: bool,
    ) -> Result<Self> {
        Self::spawn(move |worker| {
            worker.migrate(&source, &parent, &protected, force_copy)?;
            Ok("文件夹已迁移，正在监控新位置。".into())
        })
    }
    pub fn recover() -> Result<Self> {
        Self::spawn(|worker| {
            let committed = worker.load()?.is_some_and(|journal| journal.committed);
            worker.recover()?;
            Ok(if committed {
                "原文件夹已清理，正在监控新位置。"
            } else {
                "未完成的迁移已回退，继续监控原位置。"
            }
            .into())
        })
    }
    fn spawn(
        action: impl FnOnce(&mut Transfer) -> Result<String> + Send + 'static,
    ) -> Result<Self> {
        let (tx, events) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let join = thread::Builder::new().name("folder-migration".into()).spawn(move || {
            let mut transfer = Transfer { tx, cancel: flag, last_progress: Instant::now() - Duration::from_secs(1) };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| action(&mut transfer)))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("迁移任务异常中断")));
            let outcome = match result {
                Ok(message) => Outcome { message, retry: false },
                Err(error) => {
                    // Read the durable transaction outcome, not the worker's possibly stale state.
                    let recovery = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| transfer.recover()))
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("迁移恢复异常，请保留日志并重新启动软件")));
                    Outcome {
                        message: match &recovery {
                            Ok(()) => format!("{error:#}\n已恢复到安全状态。"),
                            Err(recovery) => format!("{error:#}\n尚有内容需要恢复或清理：{recovery:#}\n扫描已暂停，请重试清理。"),
                        },
                        retry: recovery.is_err(),
                    }
                }
            };
            let _ = transfer.tx.send(Request::Finished(outcome));
        })?;
        Ok(Self {
            events,
            cancel,
            join: Some(join),
        })
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn finish(&mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
// The service must keep servicing journal requests until Finished before joining.
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        // Disconnect pending database RPCs if the service itself fails.
        let (_, empty) = unbounded();
        let receiver = std::mem::replace(&mut self.events, empty);
        drop(receiver);
        self.finish();
    }
}

/// Windows-aware component comparison: `videos` must not also match `videos-old`.
pub fn relative_under(path: &Path, root: &Path) -> Option<PathBuf> {
    use std::path::{Component, Prefix};
    let component_key = |component: Component<'_>| match component {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                format!("drive:{}", drive.to_ascii_lowercase())
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
                "unc:{}\\{}",
                server.to_string_lossy().to_lowercase(),
                share.to_string_lossy().to_lowercase()
            ),
            _ => component.as_os_str().to_string_lossy().to_lowercase(),
        },
        _ => component.as_os_str().to_string_lossy().to_lowercase(),
    };
    let mut components = path.components();
    for component in root.components() {
        if component_key(components.next()?) != component_key(component) {
            return None;
        }
    }
    Some(components.collect())
}

pub fn destination(
    source: &Path,
    parent: &Path,
    protected: &[PathBuf],
) -> Result<(PathBuf, PathBuf)> {
    no_reparse_ancestors(source)?;
    no_reparse_ancestors(parent)?;
    let source = scanner::normalize(source)?;
    let parent = scanner::normalize(parent)?;
    ensure!(source.is_dir() && parent.is_dir(), "请选择可访问的文件夹");
    let name = source.file_name().context("不能迁移磁盘或共享根目录")?;
    let destination = parent.join(name);
    ensure!(
        relative_under(&destination, &source).is_none(),
        "不能原地迁移或移入自身子目录"
    );
    ensure!(
        !exists(&destination)?,
        "目标已存在同名文件或文件夹：{}，请选择其他目录",
        destination.display()
    );
    for path in protected {
        let path = scanner::normalize(path)?;
        ensure!(
            relative_under(&path, &source).is_none(),
            "不能迁移包含程序或资料库的文件夹：{}",
            path.display()
        );
    }
    Ok((source, destination))
}

pub fn answer(db: &mut Database, request: Request) -> Option<Progress> {
    match request {
        Request::Save(journal, reply) => {
            let _ = reply.send(db.save_migration(&journal).map_err(|e| format!("{e:#}")));
        }
        Request::Commit(journal, reply) => {
            let _ = reply.send(db.commit_migration(&journal).map_err(|e| format!("{e:#}")));
        }
        Request::Load(reply) => {
            let _ = reply.send(db.migration_journal().map_err(|e| format!("{e:#}")));
        }
        Request::Clear(reply) => {
            let _ = reply.send(db.clear_migration().map_err(|e| format!("{e:#}")));
        }
        Request::Progress(progress) => return Some(progress),
        Request::Finished(_) => unreachable!("completion is handled by the service"),
    }
    None
}

struct Transfer {
    tx: Sender<Request>,
    cancel: Arc<AtomicBool>,
    last_progress: Instant,
}

struct CopyContext<'a> {
    transfer: &'a mut Transfer,
    journal: &'a mut Journal,
    index: usize,
    done: u64,
    total: u64,
    error: Option<String>,
}
unsafe extern "system" fn copy_progress(
    _size: i64,
    transferred: i64,
    _stream_size: i64,
    _stream_done: i64,
    _stream: u32,
    _reason: u32,
    _source: windows_sys::Win32::Foundation::HANDLE,
    target: windows_sys::Win32::Foundation::HANDLE,
    context: *const core::ffi::c_void,
) -> u32 {
    // CopyFileEx invokes this synchronously and never retains the stack pointer.
    let context = unsafe { &mut *context.cast_mut().cast::<CopyContext<'_>>() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        context.transfer.check_cancel()?;
        if context.journal.entries[context.index]
            .destination_id
            .is_none()
        {
            context.journal.entries[context.index].destination_id = Some(handle_identity(target)?);
            context.transfer.checkpoint(context.journal, false)?;
        }
        let source = context
            .journal
            .source
            .join(&context.journal.entries[context.index].relative);
        context.transfer.progress(
            "正在复制…",
            Some(&source),
            context.done + transferred.max(0) as u64,
            context.total,
            true,
            false,
        );
        Ok(())
    }))
    .unwrap_or_else(|_| Err(anyhow::anyhow!("复制进度处理异常")));
    match result {
        Ok(()) => PROGRESS_CONTINUE,
        Err(error) => {
            context.error = Some(format!("{error:#}"));
            PROGRESS_CANCEL
        }
    }
}

fn named_streams(path: &Path) -> Result<Vec<(String, u64)>> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    let path = extended_wide(path);
    let mut data = WIN32_FIND_STREAM_DATA::default();
    let handle = unsafe {
        FindFirstStreamW(
            path.as_ptr(),
            FindStreamInfoStandard,
            (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(),
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = std::io::Error::last_os_error();
        // Filesystems without named stream support, and directories with no streams.
        if matches!(error.raw_os_error(), Some(1 | 38 | 50 | 87)) {
            return Ok(vec![]);
        }
        return Err(error.into());
    }
    struct Search(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for Search {
        fn drop(&mut self) {
            unsafe {
                FindClose(self.0);
            }
        }
    }
    let search = Search(handle);
    let mut streams = Vec::new();
    loop {
        let length = data
            .cStreamName
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(data.cStreamName.len());
        let name = String::from_utf16(&data.cStreamName[..length]).context("额外数据流名称无效")?;
        if name != "::$DATA" {
            streams.push((name, data.StreamSize.max(0) as u64));
        }
        if unsafe { FindNextStreamW(search.0, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast()) }
            == 0
        {
            let error = std::io::Error::last_os_error();
            ensure!(
                error.raw_os_error() == Some(38),
                "无法枚举额外数据流：{error}"
            );
            break;
        }
    }
    streams.sort();
    Ok(streams)
}

fn stream_file(path: &Path, name: &str) -> Result<File> {
    no_reparse_ancestors(path)?;
    let mut stream = path.as_os_str().to_os_string();
    stream.push(name);
    Ok(OpenOptions::new()
        .read(true)
        .share_mode(5)
        .open(PathBuf::from(stream))?)
}

fn streams_match(
    source: &Path,
    target: &Path,
    partial: bool,
    cancel: Option<&AtomicBool>,
) -> Result<bool> {
    let sources = named_streams(source)?;
    let targets = named_streams(target)?;
    if !partial && sources != targets {
        return Ok(false);
    }
    for (name, _) in targets {
        if !sources.iter().any(|(s, _)| s == &name) {
            return Ok(false);
        }
        let mut original = stream_file(source, &name)?;
        let mut copied = stream_file(target, &name)?;
        let equal = if partial {
            is_prefix(&mut copied, &mut original)?
        } else {
            hash_file(&mut copied, cancel)? == hash_file(&mut original, cancel)?
        };
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}

impl Transfer {
    fn checkpoint(&self, journal: &Journal, commit: bool) -> Result<()> {
        let (tx, rx) = bounded(1);
        self.tx.send(if commit {
            Request::Commit(journal.clone(), tx)
        } else {
            Request::Save(journal.clone(), tx)
        })?;
        rx.recv()?.map_err(anyhow::Error::msg)
    }
    fn load(&self) -> Result<Option<Journal>> {
        let (tx, rx) = bounded(1);
        self.tx.send(Request::Load(tx))?;
        rx.recv()?.map_err(anyhow::Error::msg)
    }
    fn clear(&self) -> Result<()> {
        let (tx, rx) = bounded(1);
        self.tx.send(Request::Clear(tx))?;
        rx.recv()?.map_err(anyhow::Error::msg)
    }
    fn check_cancel(&self) -> Result<()> {
        ensure!(!self.cancel.load(Ordering::Relaxed), "迁移已取消");
        Ok(())
    }
    fn progress(
        &mut self,
        stage: &str,
        path: Option<&Path>,
        done: u64,
        total: u64,
        cancellable: bool,
        force: bool,
    ) {
        if force || self.last_progress.elapsed() >= Duration::from_millis(100) {
            let _ = self.tx.send(Request::Progress(Progress {
                stage: stage.into(),
                path: path.map(Path::to_path_buf),
                done,
                total,
                cancellable,
            }));
            self.last_progress = Instant::now();
        }
    }
    fn migrate(
        &mut self,
        source: &Path,
        parent: &Path,
        protected: &[PathBuf],
        force_copy: bool,
    ) -> Result<()> {
        self.check_cancel()?;
        ensure!(self.load()?.is_none(), "请先恢复上次迁移");
        let (source, destination) = destination(source, parent, protected)?;
        self.progress("正在检查文件夹…", Some(&source), 0, 0, true, true);
        let entries = inventory(&source, &self.cancel)?;
        let source_id = identity(&source)?;
        let copy =
            force_copy || source_id.volume != identity(destination.parent().unwrap())?.volume;
        let mut journal = Journal {
            source,
            destination,
            staging: None,
            source_id,
            staging_id: None,
            entries,
            committed: false,
        };
        if copy {
            // Reserve an unpredictable, task-owned directory. No recursive deletion is used.
            let temp = tempfile::Builder::new()
                .prefix(".frame-migration-")
                .tempdir_in(parent)?;
            journal.staging_id = Some(identity(temp.path())?);
            journal.staging = Some(temp.path().to_path_buf());
            self.checkpoint(&journal, false)?;
            let _ = temp.keep();
            self.copy_tree(&mut journal)?;
            self.verify_tree(&journal)?;
        } else {
            self.checkpoint(&journal, false)?;
        }
        self.check_cancel()?;
        ensure!(
            inventory(&journal.source, &self.cancel)?
                .iter()
                .map(signature)
                .eq(journal.entries.iter().map(signature)),
            "源文件夹在迁移期间发生变化，未提交迁移"
        );
        self.progress("正在更新路径，请稍候…", None, 0, 0, false, true);
        let from = journal.staging.as_deref().unwrap_or(&journal.source);
        ensure_identity(from, journal.staging_id.unwrap_or(journal.source_id))?;
        rename_new(from, &journal.destination)?;
        self.checkpoint(&journal, true)?;
        journal.committed = true;
        self.cleanup(&journal)?;
        self.clear()
    }

    fn copy_tree(&mut self, journal: &mut Journal) -> Result<()> {
        let staging = journal.staging.clone().unwrap();
        let total = journal
            .entries
            .iter()
            .filter(|e| !e.directory)
            .map(|e| e.bytes)
            .sum();
        let mut done = 0;
        for index in 0..journal.entries.len() {
            self.check_cancel()?;
            let entry = journal.entries[index].clone();
            let source = journal.source.join(&entry.relative);
            let target = staging.join(&entry.relative);
            ensure_identity(&source, entry.source_id)?;
            if entry.directory {
                fs::create_dir(&target)
                    .with_context(|| format!("无法创建 {}", target.display()))?;
                journal.entries[index].destination_id = Some(identity(&target)?);
                self.checkpoint(journal, false)?;
                continue;
            }
            let mut input = read_locked(&source)?;
            ensure!(
                file_identity(&input)? == entry.source_id && unchanged(&input, &entry)?,
                "源文件已变化：{}",
                source.display()
            );
            self.progress("正在复制…", Some(&source), done, total, true, true);
            let from = extended_wide(&source);
            let to = extended_wide(&target);
            let mut context = CopyContext {
                transfer: self,
                journal,
                index,
                done,
                total,
                error: None,
            };
            let copied = unsafe {
                CopyFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    Some(copy_progress),
                    (&mut context as *mut CopyContext<'_>).cast(),
                    std::ptr::null_mut(),
                    COPY_FILE_FAIL_IF_EXISTS,
                )
            };
            if copied == 0 {
                bail!(
                    "复制失败：{}：{}",
                    source.display(),
                    context
                        .error
                        .unwrap_or_else(|| std::io::Error::last_os_error().to_string())
                );
            }
            // Native copying preserves alternate data streams and ordinary Windows attributes.
            let mut output = OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(1)
                .open(&target)?;
            journal.entries[index].destination_id = Some(file_identity(&output)?);
            output.sync_all()?;
            let expected = hash_file(&mut input, Some(&self.cancel))?;
            done += entry.bytes;
            self.progress("正在校验副本…", Some(&target), done, total, true, true);
            ensure!(
                hash_file(&mut output, Some(&self.cancel))? == expected,
                "副本校验失败：{}",
                target.display()
            );
            ensure!(
                streams_match(&source, &target, false, Some(&self.cancel))?,
                "额外数据流校验失败：{}",
                target.display()
            );
            ensure!(
                unchanged(&input, &entry)?,
                "源文件已变化：{}",
                source.display()
            );
            journal.entries[index].hash = Some(expected);
            self.checkpoint(journal, false)?;
        }
        Ok(())
    }

    fn verify_tree(&mut self, journal: &Journal) -> Result<()> {
        let staging = journal.staging.as_ref().unwrap();
        let copied = inventory(staging, &self.cancel)?;
        ensure!(
            copied.len() == journal.entries.len(),
            "临时目录内容已变化，未提交迁移"
        );
        for (actual, expected) in copied.iter().zip(&journal.entries) {
            self.check_cancel()?;
            ensure!(
                actual.relative == expected.relative
                    && actual.directory == expected.directory
                    && Some(actual.source_id) == expected.destination_id
                    && actual.size == expected.size
                    && actual.bytes == expected.bytes,
                "临时副本发生变化：{}",
                actual.relative.display()
            );
            if expected.directory {
                continue;
            }
            let source_path = journal.source.join(&expected.relative);
            let target_path = staging.join(&expected.relative);
            self.progress("正在完成整体校验…", Some(&target_path), 0, 0, true, false);
            let mut source = read_locked(&source_path)?;
            let mut target = read_locked(&target_path)?;
            let hash = expected.hash.as_ref().context("副本尚未校验")?;
            ensure!(
                file_identity(&source)? == expected.source_id
                    && unchanged(&source, expected)?
                    && hash_file(&mut source, Some(&self.cancel))? == *hash
                    && hash_file(&mut target, Some(&self.cancel))? == *hash
                    && streams_match(&source_path, &target_path, false, Some(&self.cancel))?,
                "源文件或副本在迁移期间发生变化：{}",
                source_path.display()
            );
        }
        Ok(())
    }

    fn recover(&mut self) -> Result<()> {
        let Some(journal) = self.load()? else {
            return Ok(());
        };
        self.progress(
            if journal.committed {
                "正在清理原文件夹…"
            } else {
                "正在恢复未完成的迁移…"
            },
            None,
            0,
            0,
            false,
            true,
        );
        if journal.committed {
            self.cleanup(&journal)?;
        } else if let Some(staging) = &journal.staging {
            // Promotion may have happened just before the database transaction failed.
            let location = if exists(staging)? {
                staging
            } else {
                &journal.destination
            };
            if exists(location)? {
                ensure_identity(location, journal.staging_id.context("缺少临时目录标识")?)?;
                self.remove_copies(&journal, location)?;
                fs::remove_dir(location)
                    .with_context(|| format!("请保留并检查未清理目录 {}", location.display()))?;
            }
        } else if exists(&journal.source)? && identity(&journal.source)? == journal.source_id {
            // A failed rename may leave an unrelated destination created by another process.
            // The original directory never moved; do not touch that destination.
        } else if exists(&journal.destination)? {
            ensure!(
                !exists(&journal.source)?,
                "原路径已被占用，请保留两个目录：{}；{}",
                journal.source.display(),
                journal.destination.display()
            );
            ensure_identity(&journal.destination, journal.source_id)?;
            rename_new(&journal.destination, &journal.source)?;
        } else {
            ensure_identity(&journal.source, journal.source_id)?;
        }
        self.clear()
    }

    fn remove_copies(&mut self, journal: &Journal, location: &Path) -> Result<()> {
        for entry in journal.entries.iter().rev() {
            let path = location.join(&entry.relative);
            if !exists(&path)? {
                continue;
            }
            let expected = entry
                .destination_id
                .with_context(|| format!("无法确认临时内容归属，请保留 {}", path.display()))?;
            ensure_identity(&path, expected)?;
            if entry.directory {
                fs::remove_dir(&path)
                    .with_context(|| format!("临时目录含新增内容：{}", path.display()))?;
            } else {
                let mut target = delete_locked(&path)?;
                ensure!(
                    file_identity(&target)? == expected,
                    "临时文件已被替换：{}",
                    path.display()
                );
                if let Some(hash) = &entry.hash {
                    ensure!(
                        hash_file(&mut target, None)? == *hash,
                        "临时副本发生变化，请保留 {}",
                        path.display()
                    );
                } else {
                    // A crash during a file copy: delete only a verified prefix of the unchanged source.
                    let mut source = read_locked(&journal.source.join(&entry.relative))?;
                    ensure!(
                        file_identity(&source)? == entry.source_id && unchanged(&source, entry)?,
                        "源文件已变化，无法清理不完整副本：{}",
                        path.display()
                    );
                    ensure!(
                        is_prefix(&mut target, &mut source)?,
                        "临时内容已变化：{}",
                        path.display()
                    );
                }
                ensure!(
                    streams_match(&journal.source.join(&entry.relative), &path, true, None)?,
                    "临时副本的额外数据流发生变化：{}",
                    path.display()
                );
                delete_handle(&target)?;
            }
        }
        Ok(())
    }

    fn cleanup(&mut self, journal: &Journal) -> Result<()> {
        ensure_identity(
            &journal.destination,
            journal.staging_id.unwrap_or(journal.source_id),
        )?;
        if journal.staging.is_none() {
            return Ok(());
        }
        if !exists(&journal.source)? {
            return Ok(());
        }
        ensure_identity(&journal.source, journal.source_id)?;
        self.progress(
            "正在清理原文件夹，请稍候…",
            Some(&journal.source),
            0,
            0,
            false,
            true,
        );
        for entry in journal.entries.iter().rev() {
            let source = journal.source.join(&entry.relative);
            if !exists(&source)? {
                continue;
            }
            ensure_identity(&source, entry.source_id)?;
            if entry.directory {
                fs::remove_dir(&source).with_context(|| {
                    format!("原目录含新增或被占用内容，请保留 {}", source.display())
                })?;
            } else {
                let target_path = journal.destination.join(&entry.relative);
                let mut target = read_locked(&target_path)?;
                let hash = entry.hash.as_ref().context("缺少副本校验信息")?;
                ensure!(
                    Some(file_identity(&target)?) == entry.destination_id
                        && hash_file(&mut target, None)? == *hash,
                    "目标副本已变化，原文件已保留：{}",
                    source.display()
                );
                let mut original = delete_locked(&source)?;
                ensure!(
                    file_identity(&original)? == entry.source_id
                        && unchanged(&original, entry)?
                        && hash_file(&mut original, None)? == *hash,
                    "原文件发生变化，已保留：{}",
                    source.display()
                );
                ensure!(
                    streams_match(&source, &target_path, false, None)?,
                    "额外数据流发生变化，原文件已保留：{}",
                    source.display()
                );
                delete_handle(&original)
                    .with_context(|| format!("无法清理原文件 {}", source.display()))?;
            }
        }
        fs::remove_dir(&journal.source).with_context(|| {
            format!(
                "原文件夹含新增或被占用内容，请保留 {}",
                journal.source.display()
            )
        })?;
        Ok(())
    }
}

fn signature(entry: &Entry) -> (&Path, bool, Identity, u64, i64) {
    (
        &entry.relative,
        entry.directory,
        entry.source_id,
        entry.size,
        if entry.directory { 0 } else { entry.modified },
    )
}
fn inventory(root: &Path, cancel: &AtomicBool) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
    {
        ensure!(!cancel.load(Ordering::Relaxed), "迁移已取消");
        let item = item?;
        no_reparse(item.path())?;
        let meta = item.metadata()?;
        ensure!(
            meta.is_file() || meta.is_dir(),
            "不支持的文件类型：{}",
            item.path().display()
        );
        let streams = named_streams(item.path())?;
        ensure!(
            !meta.is_dir() || streams.is_empty(),
            "目录包含额外数据流，暂不支持迁移：{}",
            item.path().display()
        );
        if item.depth() == 0 {
            continue;
        }
        entries.push(Entry {
            relative: item.path().strip_prefix(root)?.to_path_buf(),
            directory: meta.is_dir(),
            source_id: identity(item.path())?,
            destination_id: None,
            size: if meta.is_file() { meta.len() } else { 0 },
            bytes: if meta.is_file() {
                meta.len() + streams.iter().map(|(_, size)| size).sum::<u64>()
            } else {
                0
            },
            modified: scanner::modified(&meta),
            hash: None,
        });
    }
    Ok(entries)
}
fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("无法检查 {}", path.display())),
    }
}
fn no_reparse(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_attributes() & 0x400 == 0,
        "暂不支持迁移符号链接或 junction：{}",
        path.display()
    );
    Ok(())
}
fn no_reparse_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        no_reparse(ancestor)?;
    }
    Ok(())
}
fn identity(path: &Path) -> Result<Identity> {
    no_reparse_ancestors(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(7)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    file_identity(&file)
}
fn file_identity(file: &File) -> Result<Identity> {
    handle_identity(file.as_raw_handle())
}
fn handle_identity(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<Identity> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    ensure!(
        unsafe { GetFileInformationByHandle(handle, &mut info) } != 0,
        "无法读取文件标识：{}",
        std::io::Error::last_os_error()
    );
    ensure!(info.dwFileAttributes & 0x400 == 0, "不支持重解析点");
    Ok(Identity {
        volume: info.dwVolumeSerialNumber,
        index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    })
}
fn ensure_identity(path: &Path, expected: Identity) -> Result<()> {
    ensure!(
        identity(path)? == expected,
        "路径内容已被替换，请保留并检查 {}",
        path.display()
    );
    Ok(())
}
fn read_locked(path: &Path) -> Result<File> {
    no_reparse_ancestors(path)?;
    OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .with_context(|| format!("无法读取文件，可能正在写入或被占用：{}", path.display()))
}
fn delete_locked(path: &Path) -> Result<File> {
    no_reparse_ancestors(path)?;
    // GENERIC_READ | DELETE; deny other writers and replacement until disposition is set.
    OpenOptions::new()
        .access_mode(0x80000000 | 0x00010000)
        .share_mode(1)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .with_context(|| format!("文件被占用或无删除权限，已保留：{}", path.display()))
}
fn delete_handle(file: &File) -> Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            &disposition as *const _ as *const _,
            std::mem::size_of_val(&disposition) as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
fn unchanged(file: &File, entry: &Entry) -> Result<bool> {
    let meta = file.metadata()?;
    Ok(meta.len() == entry.size && scanner::modified(&meta) == entry.modified)
}
fn hash_file(file: &mut File, cancel: Option<&AtomicBool>) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        ensure!(
            !cancel.is_some_and(|c| c.load(Ordering::Relaxed)),
            "迁移已取消"
        );
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
fn is_prefix(prefix: &mut File, source: &mut File) -> Result<bool> {
    if prefix.metadata()?.len() > source.metadata()?.len() {
        return Ok(false);
    }
    let mut a = vec![0; 1024 * 1024];
    let mut b = vec![0; a.len()];
    loop {
        let count = prefix.read(&mut a)?;
        if count == 0 {
            return Ok(true);
        }
        source.read_exact(&mut b[..count])?;
        if a[..count] != b[..count] {
            return Ok(false);
        }
    }
}
fn rename_new(from: &Path, to: &Path) -> Result<()> {
    no_reparse_ancestors(from)?;
    no_reparse_ancestors(to.parent().context("目标缺少父目录")?)?;
    ensure!(!exists(to)?, "目标已存在，不会覆盖：{}", to.display());
    let from_wide = extended_wide(from);
    let to_wide = extended_wide(to);
    if unsafe { MoveFileExW(from_wide.as_ptr(), to_wide.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        bail!(
            "无法移动 {} → {}：{}",
            from.display(),
            to.display(),
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

fn extended_wide(path: &Path) -> Vec<u16> {
    let value = path.as_os_str().to_string_lossy();
    let path = if value.starts_with(r"\\?\") {
        path.to_path_buf()
    } else if let Some(unc) = value.strip_prefix(r"\\") {
        PathBuf::from(format!(r"\\?\UNC\{unc}"))
    } else {
        PathBuf::from(format!(r"\\?\{value}"))
    };
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
