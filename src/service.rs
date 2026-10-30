use crate::{
    database::Database,
    model::*,
    processing::{Executor, Pool},
    scanner::{self, ScanEvent, ScanRequest, ScanStats},
};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, unbounded};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub enum Command {
    Import(Vec<PathBuf>),
    AddRoots(Vec<PathBuf>),
    RemoveRoot(PathBuf),
    Scan(bool),
    Cancel,
    CreateTag(String),
    RenameTag(i64, String),
    DeleteTag(i64),
    Assign(Vec<i64>, TagAssignment, bool),
    Forget(Vec<i64>, bool),
    Trash(Vec<DeleteTarget>),
    Settings(Settings),
    AppSettings(AppSettings),
    UpdateSettings(UpdateSettings),
    FlushAndShutdown(Sender<Result<(), String>>),
    Shutdown,
}
#[derive(Debug, Clone)]
pub enum Event {
    Snapshot(Snapshot),
    Settings(Settings),
    AppSettings(AppSettings),
    AppSettingsSaved(Result<AppSettings, String>),
    UpdateSettings(Result<UpdateSettings, String>),
    UpdateSettingsSaved(Result<UpdateSettings, String>),
    Pending,
    Progress {
        done: usize,
        total: usize,
        bytes: u64,
    },
    Status(String),
    Error(String),
    Stopped,
}

pub struct Service {
    pub commands: Sender<Command>,
    pub events: Receiver<Event>,
    pub executor: Executor,
    join: Option<JoinHandle<()>>,
    shutdown_requested: Arc<AtomicBool>,
}
impl Service {
    pub fn start(data: PathBuf) -> Result<Self> {
        fs::create_dir_all(&data).context("无法创建 data 目录，请将软件放到可写目录")?;
        // Verify write access before creating the UI's worker.
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(data.join("application.log"))
            .context("data 目录不可写")?;
        let (commands, rx) = unbounded();
        let (tx, events) = unbounded();
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let stop = shutdown_requested.clone();
        let pool = Pool::new(AppSettings::default().concurrency)?;
        let executor = pool.executor();
        let join = thread::Builder::new()
            .name("video-database".into())
            .spawn(move || {
                let mut flushed = None;
                let result = run(data, rx, &tx, &stop, &mut flushed, pool);
                if let Err(e) = &result {
                    let _ = tx.send(Event::Error(format!("{e:#}")));
                }
                // run has returned: database, watcher, scanner and library lock are all closed.
                if let Some(reply) = flushed {
                    let _ = reply.send(result.map_err(|e| format!("{e:#}")));
                }
                let _ = tx.send(Event::Stopped);
            })?;
        Ok(Self {
            commands,
            events,
            executor,
            join: Some(join),
            shutdown_requested,
        })
    }
    pub fn send(&self, command: Command) {
        if matches!(command, Command::Shutdown) {
            self.shutdown_requested.store(true, Ordering::Relaxed);
        }
        let _ = self.commands.send(command);
    }
    pub fn shutdown(&mut self) {
        self.send(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
    pub fn shutdown_for_update(&mut self) -> Result<(), String> {
        let (tx, rx) = unbounded();
        self.commands
            .send(Command::FlushAndShutdown(tx))
            .map_err(|e| e.to_string())?;
        let result = rx
            .recv()
            .map_err(|_| "资料库服务未能确认保存完成".to_string())?;
        if let Some(join) = self.join.take() {
            join.join().map_err(|_| "资料库服务异常退出".to_string())?;
        }
        result
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ActiveScan {
    join: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    stats: Arc<ScanStats>,
}
impl ActiveScan {
    fn finish(&mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
impl Drop for ActiveScan {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.finish();
    }
}

fn run(
    data: PathBuf,
    commands: Receiver<Command>,
    events: &Sender<Event>,
    stop: &AtomicBool,
    flushed: &mut Option<Sender<Result<(), String>>>,
    mut pool: Pool,
) -> Result<()> {
    let instance_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data.join(".instance.lock"))?;
    instance_lock
        .try_lock()
        .context("此资料库已经在另一个窗口中打开，请先关闭另一个程序")?;
    let mut db = Database::open(&data.join("library.sqlite3"))?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(data.join("application.log"))?;
    let _ = events.send(Event::Settings(db.settings()?));
    let (app_settings, warning) = db.app_settings()?;
    pool.prepare(app_settings.concurrency)?;
    pool.set_limit(app_settings.concurrency);
    let _ = events.send(Event::AppSettings(app_settings));
    if let Some(warning) = warning {
        let _ = events.send(Event::Error(warning));
    }
    let _ = events.send(Event::UpdateSettings(
        db.update_settings().map_err(|e| format!("{e:#}")),
    ));
    publish(&db, events)?;
    if db.snapshot()?.videos.iter().any(VideoItem::pending) {
        let _ = events.send(Event::Pending);
    }
    let (watch_tx, watch_rx) = unbounded();
    let mut watcher: Option<RecommendedWatcher> =
        notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
            let _ = watch_tx.send(r);
        })
        .ok();
    let mut watched = Vec::<PathBuf>::new();
    update_watches(&db, &mut watcher, &mut watched, events)?;
    let (scan_tx, scan_rx) = unbounded();
    let mut active: Option<ActiveScan> = None;
    let mut rescan = true;
    let mut full = false;
    let mut explicit = Vec::new();
    let mut dirty = false;
    let mut pending = false;
    let mut last_scan = Instant::now();
    let mut last_publish = Instant::now();
    let mut last_progress = Instant::now();
    let mut change_at: Option<Instant> = None;
    let mut shutting_down = false;
    let mut settings_error = None;
    loop {
        if stop.load(Ordering::Relaxed)
            && let Some(scan) = &active
        {
            scan.cancel.store(true, Ordering::Relaxed);
        }
        match commands.recv_timeout(Duration::from_millis(75)) {
            Ok(Command::FlushAndShutdown(reply)) => {
                *flushed = Some(reply);
                if let Some(error) = settings_error.take() {
                    anyhow::bail!("设置尚未保存：{error}");
                }
                shutting_down = true;
                if let Some(a) = &active {
                    a.cancel.store(true, Ordering::Relaxed);
                }
            }
            Ok(Command::Shutdown) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                shutting_down = true;
                if let Some(a) = &active {
                    a.cancel.store(true, Ordering::Relaxed);
                }
            }
            Ok(command) => {
                let result = (|| -> Result<()> {
                    match command {
                        Command::Import(paths) => {
                            for p in paths {
                                explicit.push(scanner::normalize(&p)?);
                            }
                            rescan = true;
                        }
                        Command::AddRoots(paths) => {
                            for p in paths {
                                let p = scanner::normalize(&p)?;
                                if !p.is_dir() {
                                    anyhow::bail!("请选择文件夹");
                                }
                                db.add_root(&p)?;
                            }
                            update_watches(&db, &mut watcher, &mut watched, events)?;
                            rescan = true;
                            dirty = true;
                        }
                        Command::RemoveRoot(path) => {
                            db.remove_root(&path)?;
                            update_watches(&db, &mut watcher, &mut watched, events)?;
                            dirty = true;
                        }
                        Command::Scan(verify) => {
                            full |= verify;
                            rescan = true;
                        }
                        Command::Cancel => {
                            if let Some(a) = &active {
                                a.cancel.store(true, Ordering::Relaxed);
                            }
                            rescan = false;
                            full = false;
                            explicit.clear();
                            change_at = None;
                        }
                        Command::CreateTag(name) => {
                            db.create_tag(&name)?;
                            dirty = true;
                        }
                        Command::RenameTag(id, name) => {
                            db.rename_tag(id, &name)?;
                            dirty = true;
                        }
                        Command::DeleteTag(id) => {
                            db.delete_tag(id)?;
                            dirty = true;
                        }
                        Command::Assign(ids, tag, accept) => {
                            db.assign(&ids, tag, accept)?;
                            dirty = true;
                        }
                        Command::Forget(ids, pending_only) => {
                            db.forget(&ids, pending_only)?;
                            dirty = true;
                        }
                        Command::Trash(targets) => {
                            // Destructive operations are serialized after cancelling the scanner,
                            // so stale scan results cannot resurrect a just-deleted file.
                            if let Some(a) = &active {
                                a.cancel.store(true, Ordering::Relaxed);
                            }
                            if let Some(mut a) = active.take() {
                                a.finish();
                            }
                            while let Ok(e) = scan_rx.try_recv() {
                                apply_scan(e, &mut db, &mut dirty, &mut pending, &mut log)?;
                            }
                            for target in targets {
                                if stop.load(Ordering::Relaxed) {
                                    break;
                                }
                                if !db.matches_delete_target(&target)? {
                                    let _ = events.send(Event::Error(format!(
                                        "记录已变化，未删除：{}。请重新选择文件。",
                                        target.path.display()
                                    )));
                                    continue;
                                }
                                let found = scanner::fingerprint(
                                    &target.path,
                                    false,
                                    stop,
                                    &ScanStats::default(),
                                );
                                if !found.as_ref().is_ok_and(|f| f.hash == target.hash) {
                                    let _ = events.send(Event::Error(format!(
                                        "文件已变化或正被占用，未删除：{}。请重新扫描后确认。",
                                        target.path.display()
                                    )));
                                    continue;
                                }
                                match trash::delete(&target.path) {
                                    Ok(()) => {
                                        db.remove_file(target.file_id)?;
                                        dirty = true;
                                    }
                                    Err(e) => {
                                        let _ = events.send(Event::Error(format!(
                                            "未能移入回收站，记录已保留：{}\n{e}",
                                            target.path.display()
                                        )));
                                    }
                                }
                            }
                            let _ = events.send(Event::Status("回收站操作完成".into()));
                        }
                        Command::Settings(settings) => {
                            let saved = db.save_settings(&settings);
                            settings_error = saved.as_ref().err().map(|e| format!("{e:#}"));
                            saved?;
                        }
                        Command::AppSettings(settings) => {
                            let result = (|| -> Result<AppSettings> {
                                pool.prepare(settings.concurrency)?;
                                db.save_app_settings(&settings)?;
                                pool.set_limit(settings.concurrency);
                                Ok(settings)
                            })()
                            .map_err(|e| format!("{e:#}"));
                            let _ = events.send(Event::AppSettingsSaved(result));
                        }
                        Command::UpdateSettings(settings) => {
                            let result = db
                                .save_update_settings(&settings)
                                .map(|_| settings)
                                .map_err(|e| format!("{e:#}"));
                            let _ = events.send(Event::UpdateSettingsSaved(result));
                        }
                        Command::FlushAndShutdown(_) => unreachable!(),
                        Command::Shutdown => {}
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    let message = format!("{e:#}");
                    let _ = writeln!(log, "{message}");
                    let _ = events.send(Event::Error(message));
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
        if shutting_down {
            drop(watcher.take());
            if let Some(mut a) = active.take() {
                a.finish();
            }
            if flushed.is_some() {
                for event in scan_rx.try_iter() {
                    apply_scan(event, &mut db, &mut dirty, &mut pending, &mut log)?;
                }
            }
            break;
        }
        // A watcher queue is bounded by draining every tick, including noisy file copies.
        for event in watch_rx.try_iter() {
            match event {
                Ok(e)
                    if !matches!(e.kind, notify::EventKind::Access(_))
                        && e.paths.iter().any(|p| {
                            !p.starts_with(&data)
                                && (scanner::is_video(p) || p.is_dir() || p.extension().is_none())
                        }) =>
                {
                    change_at = Some(Instant::now())
                }
                Err(e) => {
                    let _ = writeln!(log, "文件夹监听：{e}");
                    rescan = true;
                }
                _ => {}
            }
        }
        if change_at.is_some_and(|t| t.elapsed() >= Duration::from_secs(2)) {
            rescan = true;
            change_at = None;
        }
        if last_scan.elapsed() >= Duration::from_secs(60) {
            rescan = true;
        }
        for event in scan_rx.try_iter() {
            if let ScanEvent::Retry(path) = &event {
                explicit.push(path.clone());
                continue;
            }
            if matches!(event, ScanEvent::Finished) {
                if let Some(mut a) = active.take() {
                    a.finish();
                    let cancelled = a.cancel.load(Ordering::Relaxed);
                    let deferred = a.stats.deferred.load(Ordering::Relaxed);
                    let status = if cancelled {
                        "扫描已取消".to_string()
                    } else {
                        format!(
                            "扫描完成 · {} 个文件已校验 · {deferred} 个稍后重试",
                            a.stats.done.load(Ordering::Relaxed)
                        )
                    };
                    let _ = events.send(Event::Status(status));
                    last_scan = Instant::now();
                }
                dirty = true;
                if pending {
                    let _ = events.send(Event::Pending);
                    pending = false;
                }
            } else if let Err(e) = apply_scan(event, &mut db, &mut dirty, &mut pending, &mut log) {
                let _ = events.send(Event::Error(format!("数据库更新失败：{e:#}")));
            }
        }
        if active.is_none() && rescan && !stop.load(Ordering::Relaxed) {
            let request = ScanRequest {
                roots: db.snapshot()?.roots,
                known: db.files()?,
                explicit: std::mem::take(&mut explicit),
                full,
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let stats = Arc::new(ScanStats::default());
            let (tx, c, s) = (scan_tx.clone(), cancel.clone(), stats.clone());
            let executor = pool.executor();
            let join = thread::Builder::new()
                .name("video-scan".into())
                .spawn(move || scanner::scan(request, tx, c, s, executor))?;
            active = Some(ActiveScan {
                join: Some(join),
                cancel,
                stats,
            });
            rescan = false;
            full = false;
            let _ = events.send(Event::Status("正在扫描…".into()));
            // Report activity immediately, including directory enumeration before
            // the first file count is available.
            let _ = events.send(Event::Progress {
                done: 0,
                total: 0,
                bytes: 0,
            });
        }
        if last_progress.elapsed() >= Duration::from_millis(250) {
            if let Some(a) = &active {
                let _ = events.send(Event::Progress {
                    done: a.stats.done.load(Ordering::Relaxed),
                    total: a.stats.total.load(Ordering::Relaxed),
                    bytes: a.stats.bytes.load(Ordering::Relaxed),
                });
            }
            last_progress = Instant::now();
        }
        if dirty && (active.is_none() || last_publish.elapsed() >= Duration::from_millis(500)) {
            publish(&db, events)?;
            dirty = false;
            last_publish = Instant::now();
        }
    }
    Ok(())
}

fn publish(db: &Database, events: &Sender<Event>) -> Result<()> {
    let _ = events.send(Event::Snapshot(db.snapshot()?));
    Ok(())
}
fn apply_scan(
    event: ScanEvent,
    db: &mut Database,
    dirty: &mut bool,
    pending: &mut bool,
    log: &mut fs::File,
) -> Result<()> {
    match event {
        ScanEvent::Found(f) => {
            *pending |= db.observe(&f)?;
            *dirty = true;
        }
        ScanEvent::Availability(path, a) => {
            db.availability(&path, a)?;
            *dirty = true;
        }
        ScanEvent::Warning(w) => {
            let _ = writeln!(log, "{w}");
        }
        ScanEvent::Finished | ScanEvent::Retry(_) => {}
    }
    Ok(())
}
fn update_watches(
    db: &Database,
    watcher: &mut Option<RecommendedWatcher>,
    watched: &mut Vec<PathBuf>,
    events: &Sender<Event>,
) -> Result<()> {
    if let Some(watcher) = watcher {
        for path in watched.drain(..) {
            let _ = watcher.unwatch(&path);
        }
        for root in db.snapshot()?.roots {
            if watched.iter().any(|parent| root.starts_with(parent)) {
                continue;
            }
            match watcher.watch(&root, RecursiveMode::Recursive) {
                Ok(()) => watched.push(root),
                Err(e) => {
                    let _ = events.send(Event::Status(format!(
                        "监听暂不可用，仍会定期扫描：{} ({e})",
                        root.display()
                    )));
                }
            }
        }
    }
    Ok(())
}
