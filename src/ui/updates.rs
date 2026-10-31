use super::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use video_management::update::{self, Candidate, CheckResult, Handoff};

#[derive(Default, PartialEq, Eq)]
pub(super) enum Phase {
    #[default]
    Idle,
    Checking,
    Validating,
    Preparing,
    Stopping,
}
enum Outcome {
    Checked(Result<CheckResult, String>),
    Folder(Result<PathBuf, String>),
    Progress(String),
    Ready(Result<Handoff, String>),
    Stopped(Result<(), String>),
}
struct Worker {
    cancel: Arc<AtomicBool>,
    join: JoinHandle<()>,
}
pub(super) struct UpdateUi {
    pub settings: UpdateSettings,
    pub loaded: bool,
    pub saving: bool,
    pub phase: Phase,
    pub status: String,
    pub candidate: Option<Candidate>,
    pub failed: bool,
    generation: u64,
    postponed: bool,
    prompt_pending: bool,
    tx: Sender<(u64, Outcome)>,
    rx: Receiver<(u64, Outcome)>,
    workers: Vec<Worker>,
    handoff: Option<Handoff>,
}
impl Default for UpdateUi {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self {
            settings: UpdateSettings::default(),
            loaded: false,
            saving: false,
            phase: Phase::Idle,
            status: "选择文件夹后，每次启动自动检查新版本。".into(),
            candidate: None,
            failed: false,
            generation: 0,
            postponed: false,
            prompt_pending: false,
            tx,
            rx,
            workers: vec![],
            handoff: None,
        }
    }
}
impl Drop for UpdateUi {
    fn drop(&mut self) {
        for worker in &self.workers {
            worker.cancel.store(true, Ordering::Relaxed);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join.join();
        }
        for (_, outcome) in self.rx.try_iter() {
            drop(outcome);
        }
    }
}
impl UpdateUi {
    pub fn installing(&self) -> bool {
        matches!(self.phase, Phase::Preparing | Phase::Stopping)
    }
    pub fn can_configure(&self) -> bool {
        self.loaded && !self.saving && !self.installing() && self.phase != Phase::Validating
    }
    fn invalidate(&mut self) {
        self.generation += 1;
        for worker in &self.workers {
            worker.cancel.store(true, Ordering::Relaxed);
        }
        self.candidate = None;
        self.prompt_pending = false;
        self.failed = false;
    }
    fn spawn(
        &mut self,
        work: impl FnOnce(Arc<AtomicBool>, Sender<(u64, Outcome)>, u64) -> Outcome + Send + 'static,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        match thread::Builder::new()
            .name("software-update".into())
            .spawn(move || {
                let result = work(c, tx.clone(), generation);
                let _ = tx.send((generation, result));
            }) {
            Ok(join) => self.workers.push(Worker { cancel, join }),
            Err(e) => self.fail(format!("无法启动更新任务：{e}")),
        }
    }
    fn fail(&mut self, message: String) {
        self.status = message;
        self.failed = true;
        self.phase = Phase::Idle;
    }
    pub fn settings_received(&mut self, result: Result<UpdateSettings, String>, saved: bool) {
        self.loaded = true;
        self.saving = false;
        match result {
            Ok(settings) => {
                self.settings = settings;
                self.check(false);
            }
            Err(e) => {
                self.fail(e);
                if !saved {
                    self.settings = UpdateSettings::default();
                }
            }
        }
    }
    pub fn check(&mut self, manual: bool) {
        if self.installing() || self.saving {
            return;
        }
        self.invalidate();
        if manual {
            self.postponed = false;
        }
        let Some(folder) = self.settings.folder.clone() else {
            self.phase = Phase::Idle;
            self.status = "选择文件夹后，每次启动自动检查新版本。".into();
            return;
        };
        self.phase = Phase::Checking;
        self.status = "正在检查更新…".into();
        self.spawn(move |cancel, _, _| {
            Outcome::Checked(
                update::check(&folder, update::VERSION, &cancel).map_err(|e| format!("{e:#}")),
            )
        });
    }
    pub fn folder(&mut self, path: PathBuf) {
        if !self.can_configure() {
            return;
        }
        self.invalidate();
        self.phase = Phase::Validating;
        self.status = "正在检查文件夹…".into();
        self.spawn(move |_, _, _| {
            Outcome::Folder(update::local_folder(&path).map_err(|e| format!("{e:#}")))
        });
    }
    pub fn dismiss(&mut self) {
        self.postponed = true;
        self.prompt_pending = false;
    }
}

impl App {
    pub(super) fn save_update_folder(&mut self, folder: Option<PathBuf>) {
        if !self.updates.can_configure() {
            return;
        }
        self.updates.invalidate();
        self.updates.phase = Phase::Idle;
        self.updates.saving = true;
        self.updates.status = "正在保存更新文件夹…".into();
        self.send(Command::UpdateSettings(UpdateSettings { folder }));
    }
    pub(super) fn install_update(&mut self) {
        if self.updates.phase != Phase::Idle || self.updates.saving || self.deletion.busy() {
            return;
        }
        let Some(candidate) = self.updates.candidate.clone() else {
            return;
        };
        let data = self.data.clone();
        self.updates.phase = Phase::Preparing;
        self.updates.failed = false;
        self.updates.prompt_pending = false;
        self.updates.status = "正在准备更新…".into();
        self.dialog = Some((self.main, Dialog::Updates));
        self.updates.spawn(move |cancel, tx, generation| {
            let result = (|| -> anyhow::Result<Handoff> {
                let executable = std::env::current_exe()?;
                let prepared = update::prepare(
                    &candidate,
                    executable.parent().unwrap(),
                    &data,
                    &cancel,
                    |status| {
                        let _ = tx.send((generation, Outcome::Progress(status)));
                    },
                )?;
                update::launch_helper(prepared, &cancel)
            })();
            Outcome::Ready(result.map_err(|e| format!("{e:#}")))
        });
    }
    pub(super) fn tick_updates(&mut self) -> Option<Task<Message>> {
        let events = self.updates.rx.try_iter().collect::<Vec<_>>();
        for (generation, outcome) in events {
            if generation != self.updates.generation {
                continue;
            }
            match outcome {
                Outcome::Progress(status) => self.updates.status = status,
                Outcome::Checked(Ok(result)) => {
                    self.updates.phase = Phase::Idle;
                    self.updates.status = if let Some(candidate) = &result.candidate {
                        format!("发现新版本 {}", candidate.release.version)
                    } else if result.ignored > 0 {
                        format!(
                            "未找到可用的新版本，已跳过 {} 个不完整或无效的发布清单。",
                            result.ignored
                        )
                    } else {
                        "当前已是最新版本。".into()
                    };
                    self.updates.candidate = result.candidate;
                    self.updates.prompt_pending =
                        self.updates.candidate.is_some() && !self.updates.postponed;
                }
                Outcome::Checked(Err(e)) | Outcome::Folder(Err(e)) | Outcome::Ready(Err(e)) => {
                    self.updates.fail(e)
                }
                Outcome::Folder(Ok(path)) => {
                    self.updates.phase = Phase::Idle;
                    self.save_update_folder(Some(path));
                }
                Outcome::Ready(Ok(handoff)) => {
                    self.updates.handoff = Some(handoff);
                    self.updates.phase = Phase::Stopping;
                    self.updates.status = "正在保存资料库，随后更新并重启…".into();
                    self.covers.stop();
                    self.stop_player();
                    let service = self.service.take();
                    self.updates.spawn(move |_, _, _| {
                        Outcome::Stopped(match service {
                            Some(mut service) => service.shutdown_for_update(),
                            None => Err("资料库服务未运行，未执行更新。".into()),
                        })
                    });
                }
                Outcome::Stopped(Ok(())) => {
                    if let Some(handoff) = &mut self.updates.handoff
                        && let Err(e) = handoff.commit()
                    {
                        self.updates.handoff.take();
                        self.updates.fail(format!("{e:#}"));
                        match Service::start(self.data.clone()) {
                            Ok(service) => self.service = Some(service),
                            Err(e) => self.error = Some(format!("{e:#}")),
                        }
                        continue;
                    }
                    return Some(iced::exit());
                }
                Outcome::Stopped(Err(e)) => {
                    self.updates.handoff.take();
                    self.updates.fail(format!("更新已取消：{e}"));
                    match Service::start(self.data.clone()) {
                        Ok(service) => self.service = Some(service),
                        Err(error) => self.error = Some(format!("{error:#}")),
                    }
                }
            }
        }
        let mut running = Vec::new();
        for worker in self.updates.workers.drain(..) {
            if worker.join.is_finished() {
                let _ = worker.join.join();
            } else {
                running.push(worker);
            }
        }
        self.updates.workers = running;
        if self.updates.prompt_pending
            && self.dialog.is_none()
            && !self.fullscreen
            && !self.migration.busy()
        {
            self.dialog = Some((self.main, Dialog::UpdateAvailable));
            self.updates.prompt_pending = false;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(folder: &std::path::Path) -> Candidate {
        Candidate {
            folder: folder.to_path_buf(),
            release: update::Release {
                product: update::PRODUCT.into(),
                platform: update::PLATFORM.into(),
                version: semver::Version::parse("0.2.0").unwrap(),
                archive: "release.zip".into(),
                size: 1,
                sha256: "00".repeat(32),
            },
        }
    }
    #[test]
    fn stale_check_result_does_not_replace_new_folder() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(temp.path().to_path_buf(), None, None);
        let old_generation = app.updates.generation;
        app.updates.invalidate();
        app.updates.status = "new folder check".into();
        app.updates
            .tx
            .send((
                old_generation,
                Outcome::Checked(Ok(CheckResult {
                    candidate: Some(candidate(temp.path())),
                    ignored: 0,
                })),
            ))
            .unwrap();
        app.tick_updates();
        assert!(app.updates.candidate.is_none());
        assert_eq!(app.updates.status, "new folder check");
    }
    #[test]
    fn prompt_waits_for_existing_dialog_and_later_suppresses_it() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(temp.path().to_path_buf(), None, None);
        app.dialog = Some((app.main, Dialog::Roots));
        let send_result = |app: &App| {
            app.updates
                .tx
                .send((
                    app.updates.generation,
                    Outcome::Checked(Ok(CheckResult {
                        candidate: Some(candidate(temp.path())),
                        ignored: 0,
                    })),
                ))
                .unwrap()
        };
        send_result(&app);
        app.tick_updates();
        assert!(matches!(app.dialog, Some((_, Dialog::Roots))));
        let _ = app.update(Message::Dismiss);
        app.tick_updates();
        assert!(matches!(app.dialog, Some((_, Dialog::UpdateAvailable))));
        let _ = app.update(Message::Dismiss);
        send_result(&app);
        app.tick_updates();
        assert!(app.dialog.is_none());
        assert!(app.updates.postponed);
    }
}
