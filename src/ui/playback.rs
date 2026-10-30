//! Own in-flight loads and teardown without ever waiting in the event loop.
use iced_video_player::Video;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const LOAD_TIMEOUT: Duration = Duration::from_secs(10);
// Includes cancelled loads and native teardown, not just the visible player.
// Uninterruptible OS/codec calls must not create unlimited threads/decoders.
const MAX_JOBS: usize = 4;

pub(super) struct Load {
    cancel: Arc<AtomicBool>,
    worker: JoinHandle<Result<Video, String>>,
    started: Instant,
}

impl Load {
    pub fn start(path: PathBuf) -> std::io::Result<Self> {
        Self::spawn(move |cancel| {
            let uri = video_management::media::file_uri(&path).map_err(|e| e.to_string())?;
            // No preliminary is_file(): it duplicates potentially blocking NAS
            // I/O. Let the source report the actual open/read error instead.
            Video::new_cancellable(&uri, &cancel, true)
                .map_err(|e| format!("无法播放 {}：{e}", path.display()))
        })
    }

    fn spawn(
        load: impl FnOnce(Arc<AtomicBool>) -> Result<Video, String> + Send + 'static,
    ) -> std::io::Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let worker = thread::Builder::new()
            .name("video-load".into())
            .spawn(move || load(token))?;
        Ok(Self {
            cancel,
            worker,
            started: Instant::now(),
        })
    }

    pub fn expired(&self) -> bool {
        self.started.elapsed() >= LOAD_TIMEOUT
    }
    pub fn is_finished(&self) -> bool {
        self.worker.is_finished()
    }
    fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn finish(self) -> Result<Video, String> {
        assert!(self.is_finished(), "only join a finished load");
        self.worker
            .join()
            .unwrap_or_else(|_| Err("播放组件未能加载此文件".into()))
    }
}

enum Retiring {
    Load(Load),
    Video(Box<Video>),
}

#[derive(Default)]
pub(super) struct Jobs {
    retiring: Vec<Retiring>,
}

impl Jobs {
    pub fn stop(&mut self, player: Option<Video>, loader: Option<Load>) {
        if let Some(video) = player {
            video.stop();
            self.retiring.push(Retiring::Video(Box::new(video)));
        }
        if let Some(load) = loader {
            load.cancel();
            self.retiring.push(Retiring::Load(load));
        }
        self.reap();
    }

    pub fn reap(&mut self) {
        let mut pending = Vec::new();
        for job in self.retiring.drain(..) {
            match job {
                Retiring::Load(load) if load.is_finished() => {
                    if let Ok(video) = load.finish() {
                        video.stop();
                        pending.push(Retiring::Video(Box::new(video)));
                    }
                }
                Retiring::Video(video) if video.is_stopped() => {
                    drop(video);
                }
                job => pending.push(job),
            }
        }
        self.retiring = pending;
    }

    pub fn can_start(&mut self) -> bool {
        self.reap();
        self.retiring.len() < MAX_JOBS
    }

    pub fn is_idle(&mut self) -> bool {
        self.reap();
        self.retiring.is_empty()
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        // All entries were cancelled on admission. Rust/native threads belong
        // to this process; a stuck native call cannot hold up window shutdown.
        self.reap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gstreamer::{self as gst, prelude::*};
    use std::sync::mpsc;

    #[test]
    fn cancelling_stuck_loads_is_immediate_and_bounds_retries() {
        let mut jobs = Jobs::default();
        let mut releases = Vec::new();
        for _ in 0..MAX_JOBS {
            assert!(jobs.can_start());
            let (tx, rx) = mpsc::channel::<()>();
            releases.push(tx);
            let load = Load::spawn(move |_| {
                let _ = rx.recv();
                Err("cancelled".into())
            })
            .unwrap();
            let token = load.cancel.clone();
            let start = Instant::now();
            jobs.stop(None, Some(load));
            assert!(start.elapsed() < Duration::from_millis(100));
            assert!(token.load(Ordering::Acquire));
        }
        assert!(!jobs.can_start());
        drop(releases);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !jobs.retiring.is_empty() {
            assert!(Instant::now() < deadline);
            jobs.reap();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(jobs.can_start());
    }

    #[test]
    fn load_deadline_does_not_wait_for_native_io() {
        let (tx, rx) = mpsc::channel::<()>();
        let mut load = Load::spawn(move |_| {
            let _ = rx.recv();
            Err("late result".into())
        })
        .unwrap();
        load.started = Instant::now() - LOAD_TIMEOUT;
        assert!(load.expired());
        let mut jobs = Jobs::default();
        jobs.stop(None, Some(load));
        drop(tx);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !jobs.retiring.is_empty() {
            assert!(Instant::now() < deadline);
            jobs.reap();
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn successful_result_arriving_after_cancel_is_stopped_and_reaped() {
        gst::init().unwrap();
        let pipeline = gst::parse::launch(
            "videotestsrc ! video/x-raw,format=NV12,width=16,height=16 ! appsink name=output",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let sink = pipeline
            .by_name("output")
            .unwrap()
            .downcast::<gstreamer_app::AppSink>()
            .unwrap();
        let worker_pipeline = pipeline.clone();
        let (release, resume) = mpsc::channel::<()>();
        let load = Load::spawn(move |_| {
            let video =
                Video::from_gst_pipeline(worker_pipeline, sink, None).map_err(|e| e.to_string())?;
            let _ = resume.recv_timeout(Duration::from_secs(2));
            Ok(video)
        })
        .unwrap();
        let mut jobs = Jobs::default();
        jobs.stop(None, Some(load));
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !jobs.retiring.is_empty() {
            assert!(Instant::now() < deadline);
            jobs.reap();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(pipeline.current_state(), gst::State::Null);
    }

    #[test]
    fn recycling_waits_for_release_and_times_out_without_dispatching() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = crate::ui::App::boot(dir.path().into(), None, None);
        if let Some(mut service) = app.service.take() {
            service.shutdown();
        }
        let (release, resume) = mpsc::channel::<()>();
        let load = Load::spawn(move |_| {
            let _ = resume.recv();
            Err("cancelled".into())
        })
        .unwrap();
        app.playback_jobs.stop(None, Some(load));
        let targets = vec![video_management::model::DeleteTarget {
            file_id: 1,
            path: dir.path().join("never-created.mp4"),
            hash: "test".into(),
        }];
        app.pending_trash = Some((targets.clone(), Instant::now()));
        let _ = app.update(crate::ui::Message::Tick);
        assert!(app.pending_trash.is_some());
        app.pending_trash = Some((targets, Instant::now() - Duration::from_secs(11)));
        let _ = app.update(crate::ui::Message::Tick);
        assert!(app.pending_trash.is_none());
        assert!(app.error.as_deref().unwrap().contains("未移入回收站"));
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !app.playback_jobs.is_idle() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
    }
}
