//! All potentially blocking pipeline operations run here, never under a UI lock.
use crate::{Error, Position};
use gstreamer::{self as gst, prelude::*};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const STALL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Default)]
struct Pending {
    paused: Option<bool>,
    seek: Option<(Position, bool, f64)>,
    volume: Option<f64>,
    muted: Option<bool>,
    subtitle: Option<String>,
    offset: Option<i64>,
}

#[derive(Debug)]
pub(crate) struct Control {
    pending: Mutex<Pending>,
    wake: Condvar,
    pub alive: Arc<AtomicBool>,
    pub paused: AtomicBool,
    pub eos: AtomicBool,
    pub eos_event: AtomicBool,
    pub position: AtomicU64,
    pub duration: AtomicU64,
    pub last_frame: Mutex<Instant>,
    heartbeat: Mutex<Instant>,
    error: Mutex<Option<glib::Error>>,
}

impl Control {
    pub fn new(alive: Arc<AtomicBool>, paused: bool, duration: Duration) -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(Pending {
                paused: Some(paused),
                ..Pending::default()
            }),
            wake: Condvar::new(),
            alive,
            paused: AtomicBool::new(paused),
            eos: AtomicBool::new(false),
            eos_event: AtomicBool::new(false),
            position: AtomicU64::new(0),
            duration: AtomicU64::new(duration.as_nanos() as u64),
            last_frame: Mutex::new(Instant::now()),
            heartbeat: Mutex::new(Instant::now()),
            error: Mutex::new(None),
        })
    }

    fn submit(&self, f: impl FnOnce(&mut Pending)) {
        f(&mut self.pending.lock().unwrap());
        self.wake.notify_one();
    }

    pub fn pause(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
        self.submit(|p| p.paused = Some(paused));
    }

    pub fn seek(&self, position: Position, accurate: bool, speed: f64) {
        self.eos.store(false, Ordering::Release);
        self.eos_event.store(false, Ordering::Release);
        self.submit(|p| p.seek = Some((position, accurate, speed)));
    }

    pub fn volume(&self, volume: f64, muted: bool) {
        self.submit(|p| {
            p.volume = Some(volume);
            p.muted = Some(muted);
        });
    }

    pub fn mute(&self, muted: bool) {
        self.submit(|p| p.muted = Some(muted));
    }
    pub fn subtitle(&self, uri: String) {
        self.submit(|p| p.subtitle = Some(uri));
    }
    pub fn offset(&self, offset: i64) {
        self.submit(|p| p.offset = Some(offset));
    }

    pub fn stop(&self) {
        self.alive.store(false, Ordering::Release);
        self.wake.notify_all();
    }

    pub fn fail(&self, message: impl AsRef<str>) {
        let mut error = self.error.lock().unwrap();
        if error.is_none() {
            *error = Some(glib::Error::new(gst::CoreError::Failed, message.as_ref()));
        }
    }

    pub fn take_error(&self) -> Option<glib::Error> {
        if self.alive.load(Ordering::Acquire)
            && self.heartbeat.lock().unwrap().elapsed() > CONTROL_TIMEOUT
        {
            self.fail("播放组件响应超时，请重试或检查文件连接");
            self.stop();
        }
        self.error.lock().unwrap().take()
    }

    pub fn run(self: Arc<Self>, pipeline: gst::Pipeline) {
        // The pipeline clone remains on this worker through teardown. The UI
        // can release its handle without running a native finalizer or join.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_inner(&pipeline)));
        match result {
            Ok(Err(error)) => self.fail(error.to_string()),
            Err(_) => self.fail("播放组件运行异常"),
            _ => {}
        }
        self.stop();
        if let Err(error) = pipeline.set_state(gst::State::Null) {
            self.fail(error.to_string());
        }
    }

    fn run_inner(&self, pipeline: &gst::Pipeline) -> Result<(), Error> {
        let bus = pipeline.bus().ok_or(Error::Bus)?;
        let mut last_position = 0;
        let mut progress = Instant::now();
        let mut queried = Instant::now() - Duration::from_secs(1);
        while self.alive.load(Ordering::Acquire) {
            let pending = {
                let mut commands = self.pending.lock().unwrap();
                std::mem::take(&mut *commands)
            };
            // No Rust mutex is held across GStreamer calls. Multiple slider
            // events coalesce; a slow decoder cannot build an unbounded queue.
            if let Some(uri) = pending.subtitle {
                pipeline.set_state(gst::State::Ready)?;
                pipeline.set_property("suburi", uri);
                pipeline.set_state(if self.paused.load(Ordering::Acquire) {
                    gst::State::Paused
                } else {
                    gst::State::Playing
                })?;
            }
            if let Some(volume) = pending.volume {
                pipeline.set_property("volume", volume);
            }
            if let Some(muted) = pending.muted {
                pipeline.set_property("mute", muted);
            }
            if let Some(offset) = pending.offset {
                pipeline.set_property("av-offset", offset);
            }
            if let Some((position, accurate, speed)) = pending.seek {
                let flags = gst::SeekFlags::FLUSH
                    | if accurate {
                        gst::SeekFlags::ACCURATE
                    } else {
                        gst::SeekFlags::KEY_UNIT
                    };
                let position = gst::GenericFormattedValue::from(position);
                let (start, end_type, end) = if speed > 0.0 {
                    (
                        position,
                        gst::SeekType::None,
                        gst::GenericFormattedValue::new(position.format(), -1),
                    )
                } else {
                    (
                        gst::GenericFormattedValue::new(position.format(), 0),
                        gst::SeekType::Set,
                        position,
                    )
                };
                pipeline.seek(speed, flags, gst::SeekType::Set, start, end_type, end)?;
                progress = Instant::now();
            }
            if !self.alive.load(Ordering::Acquire) {
                break;
            }
            if let Some(paused) = pending.paused {
                pipeline.set_state(if paused {
                    gst::State::Paused
                } else {
                    gst::State::Playing
                })?;
                progress = Instant::now();
            }
            // Drain on this worker even when the widget is hidden or paused.
            for _ in 0..64 {
                let Some(message) = bus.pop() else {
                    break;
                };
                match message.view() {
                    gst::MessageView::Error(error) => return Err(error.error().into()),
                    gst::MessageView::Eos(_) => {
                        self.eos.store(true, Ordering::Release);
                        self.eos_event.store(true, Ordering::Release);
                    }
                    _ => {}
                }
            }
            if queried.elapsed() >= Duration::from_millis(100) {
                if let Some(position) = pipeline.query_position::<gst::ClockTime>() {
                    self.position.store(position.nseconds(), Ordering::Release);
                    if position.nseconds() != last_position {
                        progress = Instant::now();
                        last_position = position.nseconds();
                    }
                }
                if let Some(duration) = pipeline.query_duration::<gst::ClockTime>() {
                    self.duration.store(duration.nseconds(), Ordering::Release);
                }
                queried = Instant::now();
            }
            if self.paused.load(Ordering::Acquire) || self.eos.load(Ordering::Acquire) {
                progress = Instant::now();
            } else if progress.elapsed() > STALL_TIMEOUT
                && self.last_frame.lock().unwrap().elapsed() > STALL_TIMEOUT
            {
                self.fail("视频读取超时，请检查文件或网络连接后重试");
                break;
            }
            *self.heartbeat.lock().unwrap() = Instant::now();
            let commands = self.pending.lock().unwrap();
            let _ = self
                .wake
                .wait_timeout(commands, Duration::from_millis(20))
                .unwrap();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresponsive_native_operation_reports_timeout_without_waiting() {
        let control = Control::new(Arc::new(AtomicBool::new(true)), false, Duration::ZERO);
        *control.heartbeat.lock().unwrap() =
            Instant::now() - CONTROL_TIMEOUT - Duration::from_secs(1);
        let start = Instant::now();
        assert!(control.take_error().unwrap().message().contains("响应超时"));
        assert!(start.elapsed() < Duration::from_millis(100));
        assert!(!control.alive.load(Ordering::Acquire));
        assert!(control.take_error().is_none());
    }
}
