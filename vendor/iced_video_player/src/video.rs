use crate::Error;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_app::prelude::*;
use gstreamer_video::{VideoFrameRef, VideoInfo, prelude::*};
use iced::widget::image as img;
use std::num::NonZeroU8;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

struct FrameWorkerGuard(Arc<crate::control::Control>);
impl Drop for FrameWorkerGuard {
    fn drop(&mut self) {
        if self.0.alive.load(Ordering::Acquire) {
            self.0.fail("视频画面处理异常，请重新播放");
            self.0.stop();
        }
    }
}

/// Position in the media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Position {
    /// Position based on time.
    ///
    /// Not the most accurate format for videos.
    Time(Duration),
    /// Position based on nth frame.
    Frame(u64),
}

impl From<Position> for gst::GenericFormattedValue {
    fn from(pos: Position) -> Self {
        match pos {
            Position::Time(t) => gst::ClockTime::from_nseconds(t.as_nanos() as _).into(),
            Position::Frame(f) => gst::format::Default::from_u64(f).into(),
        }
    }
}

impl From<Duration> for Position {
    fn from(t: Duration) -> Self {
        Position::Time(t)
    }
}

impl From<u64> for Position {
    fn from(f: u64) -> Self {
        Position::Frame(f)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Frame {
    pub(crate) bytes: Arc<[u8]>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    #[cfg(test)]
    pts: Option<gst::ClockTime>,
}

impl Frame {
    fn from_sample(sample: &gst::Sample) -> Result<Self, Error> {
        let info = VideoInfo::from_caps(sample.caps().ok_or(Error::Caps)?)?;
        let (width, height) = (info.width(), info.height());
        if info.format() != gstreamer_video::VideoFormat::Nv12
            || width == 0
            || height == 0
            || width > 16384
            || height > 16384
            || u64::from(width) * u64::from(height) > 67_108_864
        {
            return Err(Error::Caps);
        }
        let buffer = sample.buffer().ok_or(Error::Caps)?;
        let frame = VideoFrameRef::from_buffer_ref_readable(buffer, &info)?;
        let stride = width.next_multiple_of(2) as usize;
        let mut bytes = vec![0; stride * (height + height.div_ceil(2)) as usize];
        // Map/copy on the decoding worker. The UI only sees CPU-owned bytes,
        // never driver memory or a GStreamer buffer lock. Respect both planes'
        // strides and offsets, including padding and odd frame dimensions.
        for (plane, rows, row_bytes, offset) in [
            (0, height as usize, width as usize, 0),
            (
                1,
                height.div_ceil(2) as usize,
                stride,
                stride * height as usize,
            ),
        ] {
            let data = frame.plane_data(plane)?;
            let source_stride =
                usize::try_from(frame.plane_stride()[plane as usize]).map_err(|_| Error::Caps)?;
            if source_stride < row_bytes || data.len() < source_stride * (rows - 1) + row_bytes {
                return Err(Error::Caps);
            }
            for y in 0..rows {
                bytes[offset + y * stride..offset + y * stride + row_bytes]
                    .copy_from_slice(&data[y * source_stride..y * source_stride + row_bytes]);
            }
        }
        Ok(Self {
            bytes: bytes.into(),
            width,
            height,
            #[cfg(test)]
            pts: buffer.pts(),
        })
    }
}

#[derive(Debug)]
pub(crate) struct Internal {
    pub(crate) id: u64,

    pub(crate) source: gst::Pipeline,
    pub(crate) alive: Arc<AtomicBool>,
    pub(crate) worker: Option<std::thread::JoinHandle<()>>,
    pub(crate) controller: Option<std::thread::JoinHandle<()>>,
    pub(crate) control: Arc<crate::control::Control>,
    volume: f64,
    muted: bool,
    subtitle_url: Option<url::Url>,

    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) framerate: f64,
    pub(crate) speed: f64,
    pub(crate) sync_av: bool,

    pub(crate) frame: Arc<Mutex<Frame>>,
    pub(crate) upload_frame: Arc<AtomicBool>,
    pub(crate) last_frame_time: Arc<Mutex<Instant>>,
    pub(crate) looping: bool,
    pub(crate) sync_av_avg: u64,
    pub(crate) sync_av_counter: u64,

    pub(crate) subtitle_text: Arc<Mutex<Option<String>>>,
    pub(crate) upload_text: Arc<AtomicBool>,
}

impl Internal {
    pub(crate) fn seek(&self, position: impl Into<Position>, accurate: bool) -> Result<(), Error> {
        self.control.seek(position.into(), accurate, self.speed);
        *self.subtitle_text.lock().map_err(|_| Error::Lock)? = None;
        self.upload_text.store(true, Ordering::Release);
        Ok(())
    }

    pub(crate) fn set_speed(&mut self, speed: f64) -> Result<(), Error> {
        if !speed.is_finite() || speed == 0.0 {
            return Err(Error::Duration);
        }
        self.speed = speed;
        self.seek(
            Duration::from_nanos(self.control.position.load(Ordering::Acquire)),
            true,
        )
    }

    pub(crate) fn restart_stream(&mut self) -> Result<(), Error> {
        self.seek(Duration::ZERO, false)?;
        self.control.pause(false);
        Ok(())
    }

    pub(crate) fn set_paused(&mut self, paused: bool) {
        if self.control.eos.load(Ordering::Acquire) && !paused {
            let _ = self.restart_stream();
        } else {
            self.control.pause(paused);
        }
    }

    pub(crate) fn paused(&self) -> bool {
        self.control.paused.load(Ordering::Acquire)
    }

    pub(crate) fn set_av_offset(&mut self, offset: Duration) {
        if self.sync_av {
            self.sync_av_counter += 1;
            self.sync_av_avg = self.sync_av_avg * (self.sync_av_counter - 1) / self.sync_av_counter
                + offset.as_nanos() as u64 / self.sync_av_counter;
            if self.sync_av_counter % 128 == 0 {
                self.control.offset(-(self.sync_av_avg as i64));
            }
        }
    }
}
/// A multimedia video loaded from a URI (e.g., a local file path or HTTP stream).
#[derive(Debug)]
pub struct Video(pub(crate) RwLock<Internal>);

impl Drop for Video {
    fn drop(&mut self) {
        let inner = self.0.get_mut().unwrap_or_else(|e| e.into_inner());
        inner.control.stop();
        // Never join a native decoder from the event/render thread. Its worker
        // retains the pipeline through NULL and finalization. Application code
        // retains retiring videos and bounds the number of outstanding jobs.
        for worker in [&mut inner.worker, &mut inner.controller] {
            if worker.as_ref().is_some_and(|w| w.is_finished()) {
                let _ = worker.take().unwrap().join();
            }
        }
    }
}
impl Video {
    /// Create a new video player from a given video which loads from `uri`.
    /// Note that live sources will report the duration to be zero.
    pub fn new(uri: &url::Url) -> Result<Self, Error> {
        Self::new_cancellable(uri, &AtomicBool::new(false), false)
    }

    /// Load silently in the background; cancellation is checked around every
    /// native operation and while waiting for preroll. `paused` avoids audio
    /// from a cancelled/replaced UI request before it has been accepted.
    pub fn new_cancellable(
        uri: &url::Url,
        cancel: &AtomicBool,
        paused: bool,
    ) -> Result<Self, Error> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        gst::init()?;

        let pipeline = gst::parse::launch(
            "playbin text-sink=\"appsink name=iced_text sync=true drop=true\" video-sink=\"videoscale ! videoconvert ! appsink name=iced_video drop=true caps=video/x-raw,format=NV12,pixel-aspect-ratio=1/1\""
        )?
            .downcast::<gst::Pipeline>()
            .map_err(|_| Error::Cast)?;
        pipeline.set_property("uri", uri.as_str());
        pipeline.connect_deep_element_added(|_, _, element| {
            if element
                .factory()
                .is_some_and(|f| f.name().starts_with("avdec_"))
                && element.has_property("max-threads", Some(i32::static_type()))
            {
                // Avoid CPU-count-sized reference-frame pools alongside covers.
                element.set_property("max-threads", 4i32);
            }
        });

        let video_sink: gst::Element = pipeline.property("video-sink");
        let pad = video_sink.pads().first().cloned().unwrap();
        let pad = pad.dynamic_cast::<gst::GhostPad>().unwrap();
        let bin = pad
            .parent_element()
            .unwrap()
            .downcast::<gst::Bin>()
            .unwrap();
        let video_sink = bin.by_name("iced_video").unwrap();
        let video_sink = video_sink.downcast::<gst_app::AppSink>().unwrap();

        let text_sink: gst::Element = pipeline.property("text-sink");
        let text_sink = text_sink.downcast::<gst_app::AppSink>().unwrap();

        Self::from_pipeline(pipeline, video_sink, Some(text_sink), cancel, paused)
    }

    /// Creates a new video based on an existing GStreamer pipeline and appsink.
    /// Expects an `appsink` plugin with `caps=video/x-raw,format=NV12`.
    ///
    /// An optional `text_sink` can be provided, which enables subtitle messages
    /// to be emitted.
    ///
    /// **Note:** Many functions of [`Video`] assume a `playbin` pipeline.
    /// Non-`playbin` pipelines given here may not have full functionality.
    pub fn from_gst_pipeline(
        pipeline: gst::Pipeline,
        video_sink: gst_app::AppSink,
        text_sink: Option<gst_app::AppSink>,
    ) -> Result<Self, Error> {
        Self::from_pipeline(
            pipeline,
            video_sink,
            text_sink,
            &AtomicBool::new(false),
            false,
        )
    }

    fn from_pipeline(
        pipeline: gst::Pipeline,
        video_sink: gst_app::AppSink,
        text_sink: Option<gst_app::AppSink>,
        cancel: &AtomicBool,
        paused: bool,
    ) -> Result<Self, Error> {
        gst::init()?;
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);

        // We need to ensure we stop the pipeline if we hit an error,
        // or else there may be audio left playing in the background.
        macro_rules! cleanup {
            ($expr:expr) => {
                $expr.map_err(|e| {
                    let _ = pipeline.set_state(gst::State::Null);
                    e
                })
            };
        }

        // Bound decoded frames even if the consumer stalls. `drop` alone has
        // no effect with appsink's default max-buffers=0 (unlimited).
        video_sink.set_max_buffers(2);
        video_sink.set_drop(true);
        video_sink.set_enable_last_sample(false);
        video_sink.set_wait_on_eos(false);
        if let Some(sink) = &text_sink {
            sink.set_max_buffers(2);
            sink.set_drop(true);
            sink.set_enable_last_sample(false);
            sink.set_wait_on_eos(false);
        }

        // Preroll without starting audio. A state wait returning Async is NOT
        // successful loading; require an actual decoded frame, or an error.
        let deadline = Instant::now() + Duration::from_secs(10);
        cleanup!(pipeline.set_state(gst::State::Paused))?;
        let bus = cleanup!(pipeline.bus().ok_or(Error::Bus))?;
        let first = cleanup!((|| {
            loop {
                if cancel.load(Ordering::Acquire) {
                    return Err(Error::Cancelled);
                }
                if Instant::now() >= deadline {
                    return Err(Error::Timeout);
                }
                for _ in 0..64 {
                    let Some(message) = bus.pop() else {
                        break;
                    };
                    if let gst::MessageView::Error(error) = message.view() {
                        return Err(Error::Glib(error.error()));
                    }
                }
                if let Some(sample) = video_sink.try_pull_preroll(gst::ClockTime::from_mseconds(25))
                {
                    return Ok(sample);
                }
                if video_sink.is_eos() {
                    return Err(Error::Caps);
                }
            }
        })())?;

        // extract resolution and framerate
        // TODO(jazzfool): maybe we want to extract some other information too?
        let caps = cleanup!(first.caps().ok_or(Error::Caps))?;
        let s = cleanup!(caps.structure(0).ok_or(Error::Caps))?;
        let width = cleanup!(s.get::<i32>("width").map_err(|_| Error::Caps))?;
        let height = cleanup!(s.get::<i32>("height").map_err(|_| Error::Caps))?;
        if width <= 0 || height <= 0 || width > 16384 || height > 16384 {
            let _ = pipeline.set_state(gst::State::Null);
            return Err(Error::Caps);
        }
        // GStreamer uses 0/1 (or an omitted field) for variable/unknown frame
        // rates. The appsink clock schedules frames using buffer timestamps,
        // so playback does not need a positive nominal rate.
        let framerate = cleanup!(
            s.get_optional::<gst::Fraction>("framerate")
                .map_err(|_| Error::Caps)
        )?
        .map_or(0.0, |rate| rate.numer() as f64 / rate.denom() as f64);

        if !framerate.is_finite() || framerate < 0.0 {
            let _ = pipeline.set_state(gst::State::Null);
            return Err(Error::Framerate(framerate));
        }

        let duration = Duration::from_nanos(
            pipeline
                .query_duration::<gst::ClockTime>()
                .map(|duration| duration.nseconds())
                .unwrap_or(0),
        );

        let sync_av = pipeline.has_property("av-offset", None);

        // NV12 = 12bpp
        let frame = Arc::new(Mutex::new(cleanup!(Frame::from_sample(&first))?));
        let upload_frame = Arc::new(AtomicBool::new(true));
        let alive = Arc::new(AtomicBool::new(true));
        let control = crate::control::Control::new(alive.clone(), paused, duration);
        let last_frame_time = Arc::new(Mutex::new(Instant::now()));

        let frame_ref = Arc::clone(&frame);
        let upload_frame_ref = Arc::clone(&upload_frame);
        let alive_ref = Arc::clone(&alive);
        let last_frame_time_ref = Arc::clone(&last_frame_time);

        let subtitle_text = Arc::new(Mutex::new(None));
        let upload_text = Arc::new(AtomicBool::new(false));
        let subtitle_text_ref = Arc::clone(&subtitle_text);
        let upload_text_ref = Arc::clone(&upload_text);

        let frame_control = control.clone();

        let worker = cleanup!(
            std::thread::Builder::new()
                .name("video-frames".into())
                .spawn(move || {
                    let _guard = FrameWorkerGuard(frame_control.clone());
                    let mut clear_subtitles_at = None;

                    while alive_ref.load(Ordering::Acquire) {
                        let iteration = Instant::now();
                        if let Err(gst::FlowError::Error) = (|| -> Result<(), gst::FlowError> {
                            let sample = if frame_control.paused.load(Ordering::Acquire) {
                                video_sink
                                    .try_pull_preroll(gst::ClockTime::from_mseconds(16))
                                    .ok_or(gst::FlowError::Eos)?
                            } else {
                                video_sink
                                    .try_pull_sample(gst::ClockTime::from_mseconds(16))
                                    .ok_or(gst::FlowError::Eos)?
                            };

                            let frame =
                                Frame::from_sample(&sample).map_err(|_| gst::FlowError::Error)?;
                            *last_frame_time_ref
                                .lock()
                                .map_err(|_| gst::FlowError::Error)? = Instant::now();
                            *frame_control
                                .last_frame
                                .lock()
                                .map_err(|_| gst::FlowError::Error)? = Instant::now();

                            {
                                let mut frame_guard =
                                    frame_ref.lock().map_err(|_| gst::FlowError::Error)?;
                                *frame_guard = frame;
                            }

                            upload_frame_ref.swap(true, Ordering::SeqCst);

                            // Subtitle metadata is optional and must never prevent
                            // an otherwise decoded image from being displayed.
                            let Some(frame_segment) = sample.segment() else {
                                return Ok(());
                            };
                            let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                            let frame_pts = buffer.pts().unwrap_or(gst::ClockTime::ZERO);
                            let frame_duration = buffer.duration().unwrap_or(gst::ClockTime::ZERO);

                            if let Some(at) = clear_subtitles_at {
                                if frame_pts >= at {
                                    *subtitle_text_ref
                                        .lock()
                                        .map_err(|_| gst::FlowError::Error)? = None;
                                    upload_text_ref.store(true, Ordering::SeqCst);
                                    clear_subtitles_at = None;
                                }
                            }

                            let text = text_sink.as_ref().and_then(|sink| {
                                sink.try_pull_sample(gst::ClockTime::from_seconds(0))
                            });
                            if let Some(text) = text {
                                let _ = (|| -> Result<(), gst::FlowError> {
                                    let text_segment =
                                        text.segment().ok_or(gst::FlowError::Error)?;
                                    let text = text.buffer().ok_or(gst::FlowError::Error)?;
                                    let text_pts = text.pts().ok_or(gst::FlowError::Error)?;
                                    let text_duration =
                                        text.duration().unwrap_or(gst::ClockTime::ZERO);

                                    let frame_running_time =
                                        frame_segment.to_running_time(frame_pts).value();
                                    let frame_running_time_end = frame_segment
                                        .to_running_time(frame_pts.saturating_add(frame_duration))
                                        .value();

                                    let text_running_time =
                                        text_segment.to_running_time(text_pts).value();
                                    let text_running_time_end = text_segment
                                        .to_running_time(text_pts.saturating_add(text_duration))
                                        .value();

                                    // see gst-plugins-base/ext/pango/gstbasetextoverlay.c (gst_base_text_overlay_video_chain)
                                    // as an example of how to correctly synchronize the text+video segments
                                    if text_running_time_end > frame_running_time
                                        && (text_running_time <= frame_running_time
                                            || frame_running_time_end > text_running_time)
                                    {
                                        let duration =
                                            text.duration().unwrap_or(gst::ClockTime::ZERO);
                                        let map = text
                                            .map_readable()
                                            .map_err(|_| gst::FlowError::Error)?;

                                        let text = std::str::from_utf8(map.as_slice())
                                            .map_err(|_| gst::FlowError::Error)?
                                            .to_string();
                                        *subtitle_text_ref
                                            .lock()
                                            .map_err(|_| gst::FlowError::Error)? = Some(text);
                                        upload_text_ref.store(true, Ordering::SeqCst);

                                        clear_subtitles_at =
                                            Some(text_pts.saturating_add(duration));
                                    }
                                    Ok(())
                                })();
                            }

                            Ok(())
                        })() {
                            frame_control.fail("无法读取视频画面");
                            break;
                        }
                        // EOS/stopped appsinks return immediately. Never spin at 100%
                        // CPU; no pipeline state query (and its native locks) is needed.
                        std::thread::sleep(
                            Duration::from_millis(8).saturating_sub(iteration.elapsed()),
                        );
                    }
                })
        )?;

        let controller_ref = control.clone();
        let pipeline_ref = pipeline.clone();
        let controller = match std::thread::Builder::new()
            .name("video-control".into())
            .spawn(move || controller_ref.run(pipeline_ref))
        {
            Ok(worker) => worker,
            Err(error) => {
                control.stop();
                let _ = pipeline.set_state(gst::State::Null);
                let _ = worker.join();
                return Err(error.into());
            }
        };

        Ok(Video(RwLock::new(Internal {
            id,

            source: pipeline,
            alive,
            worker: Some(worker),
            controller: Some(controller),
            control,
            volume: 1.0,
            muted: false,
            subtitle_url: None,

            width,
            height,
            framerate,
            speed: 1.0,
            sync_av,

            frame,
            upload_frame,
            last_frame_time,
            looping: false,
            sync_av_avg: 0,
            sync_av_counter: 0,

            subtitle_text,
            upload_text,
        })))
    }

    pub(crate) fn read(&self) -> impl Deref<Target = Internal> + '_ {
        self.0.read().expect("lock")
    }

    pub(crate) fn write(&self) -> impl DerefMut<Target = Internal> + '_ {
        self.0.write().expect("lock")
    }

    pub(crate) fn get_mut(&mut self) -> impl DerefMut<Target = Internal> + '_ {
        self.0.get_mut().expect("lock")
    }

    /// Get the size/resolution of the video as `(width, height)`.
    pub fn size(&self) -> (i32, i32) {
        let inner = self.read();
        let frame = inner.frame.lock().unwrap();
        (frame.width as i32, frame.height as i32)
    }

    /// Get the nominal framerate as frames per second, or 0 for variable/unknown rates.
    pub fn framerate(&self) -> f64 {
        self.read().framerate
    }

    /// Set the volume multiplier of the audio.
    /// `0.0` = 0% volume, `1.0` = 100% volume.
    ///
    /// This uses a linear scale, for example `0.5` is perceived as half as loud.
    pub fn set_volume(&mut self, volume: f64) {
        if !volume.is_finite() {
            return;
        }
        let mut inner = self.get_mut();
        inner.volume = volume.clamp(0.0, 10.0);
        inner.control.volume(inner.volume, inner.muted);
    }

    /// Get the volume multiplier of the audio.
    pub fn volume(&self) -> f64 {
        self.read().volume
    }

    /// Set if the audio is muted or not, without changing the volume.
    pub fn set_muted(&mut self, muted: bool) {
        let mut inner = self.get_mut();
        inner.muted = muted;
        inner.control.mute(muted);
    }

    /// Get if the audio is muted or not.
    pub fn muted(&self) -> bool {
        self.read().muted
    }

    /// Get if the stream ended or not.
    pub fn eos(&self) -> bool {
        self.read().control.eos.load(Ordering::Acquire)
    }

    /// Get if the media will loop or not.
    pub fn looping(&self) -> bool {
        self.read().looping
    }

    /// Set if the media will loop or not.
    pub fn set_looping(&mut self, looping: bool) {
        self.get_mut().looping = looping;
    }

    /// Set if the media is paused or not.
    pub fn set_paused(&mut self, paused: bool) {
        self.get_mut().set_paused(paused)
    }

    /// Get if the media is paused or not.
    pub fn paused(&self) -> bool {
        self.read().paused()
    }

    /// Jumps to a specific position in the media.
    /// Passing `true` to the `accurate` parameter will result in more accurate seeking,
    /// however, it is also slower. For most seeks (e.g., scrubbing) this is not needed.
    pub fn seek(&mut self, position: impl Into<Position>, accurate: bool) -> Result<(), Error> {
        self.get_mut().seek(position, accurate)
    }

    /// Set the playback speed of the media.
    /// The default speed is `1.0`.
    pub fn set_speed(&mut self, speed: f64) -> Result<(), Error> {
        self.get_mut().set_speed(speed)
    }

    /// Get the current playback speed.
    pub fn speed(&self) -> f64 {
        self.read().speed
    }

    /// Get the current playback position in time.
    pub fn position(&self) -> Duration {
        Duration::from_nanos(self.read().control.position.load(Ordering::Acquire))
    }

    /// Get the media duration.
    pub fn duration(&self) -> Duration {
        Duration::from_nanos(self.read().control.duration.load(Ordering::Acquire))
    }

    /// Restarts a stream; seeks to the first frame and unpauses, sets the `eos` flag to false.
    pub fn restart_stream(&mut self) -> Result<(), Error> {
        self.get_mut().restart_stream()
    }

    /// Set the subtitle URL to display.
    pub fn set_subtitle_url(&mut self, url: &url::Url) -> Result<(), Error> {
        let mut inner = self.get_mut();
        inner.subtitle_url = Some(url.clone());
        inner.control.subtitle(url.as_str().into());
        Ok(())
    }

    /// Get the current subtitle URL.
    pub fn subtitle_url(&self) -> Option<url::Url> {
        self.read().subtitle_url.clone()
    }

    /// Request asynchronous shutdown. Never blocks on decoder or file I/O.
    pub fn stop(&self) {
        self.read().control.stop();
    }

    /// Whether both owned workers have completed, including native teardown.
    pub fn is_stopped(&self) -> bool {
        let inner = self.read();
        [&inner.worker, &inner.controller]
            .iter()
            .all(|w| w.as_ref().is_none_or(|w| w.is_finished()))
    }

    /// Explicit bounded wait for command-line callers/tests, never used by UI.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        self.stop();
        let deadline = Instant::now() + timeout;
        while !self.is_stopped() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    /// Poll errors independently of widget visibility (including watchdog).
    pub fn take_error(&self) -> Option<glib::Error> {
        self.read().control.take_error()
    }

    /// Get the underlying GStreamer pipeline.
    pub fn pipeline(&self) -> gst::Pipeline {
        self.read().source.clone()
    }

    /// Generates a list of thumbnails based on a set of positions in the media, downscaled by a given factor.
    ///
    /// Slow; only needs to be called once for each instance.
    /// It's best to call this at the very start of playback, otherwise the position may shift.
    pub fn thumbnails<I>(
        &mut self,
        positions: I,
        downscale: NonZeroU8,
    ) -> Result<Vec<img::Handle>, Error>
    where
        I: IntoIterator<Item = Position>,
    {
        let downscale = u8::from(downscale) as u32;

        let paused = self.paused();
        let muted = self.muted();
        let pos = self.position();

        self.set_paused(false);
        self.set_muted(true);

        let out = {
            let inner = self.read();
            let width = inner.width;
            let height = inner.height;
            positions
                .into_iter()
                .map(|pos| {
                    inner.seek(pos, true)?;
                    inner.upload_frame.store(false, Ordering::SeqCst);
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while !inner.upload_frame.load(Ordering::SeqCst) {
                        if Instant::now() >= deadline || !inner.alive.load(Ordering::Acquire) {
                            return Err(Error::Timeout);
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let frame_guard = inner.frame.lock().map_err(|_| Error::Lock)?;
                    let frame = &frame_guard.bytes;
                    let stride = Some(frame_guard.width.next_multiple_of(2));

                    Ok(img::Handle::from_rgba(
                        inner.width as u32 / downscale,
                        inner.height as u32 / downscale,
                        yuv_to_rgba(frame.as_ref(), width as _, height as _, downscale, stride),
                    ))
                })
                .collect()
        };

        self.set_paused(paused);
        self.set_muted(muted);
        self.seek(pos, true)?;

        out
    }
}

fn yuv_to_rgba(
    yuv: &[u8],
    width: u32,
    height: u32,
    downscale: u32,
    stride: Option<u32>,
) -> Vec<u8> {
    // Use stride from VideoMeta if available, otherwise assume stride == width
    let stride = stride.unwrap_or(width);

    let uv_start = stride * height;
    let mut rgba = vec![];

    for y in 0..height / downscale {
        for x in 0..width / downscale {
            let x_src = x * downscale;
            let y_src = y * downscale;

            // NV12 memory layout with stride:
            // Y plane: stride bytes per row, starting at offset 0
            // UV plane: stride bytes per row (same stride), starting at offset stride * height
            // Each pixel is 1 byte Y, and every 2x2 block shares 2 bytes (U, V)
            let y_offset = (y_src * stride + x_src) as usize;
            let uv_offset = (uv_start + (y_src / 2) * stride + (x_src / 2) * 2) as usize;

            let y = yuv[y_offset] as f32;
            let u = yuv[uv_offset] as f32;
            let v = yuv[uv_offset + 1] as f32;

            let r = 1.164 * (y - 16.0) + 1.596 * (v - 128.0);
            let g = 1.164 * (y - 16.0) - 0.813 * (v - 128.0) - 0.391 * (u - 128.0);
            let b = 1.164 * (y - 16.0) + 2.018 * (u - 128.0);

            rgba.push(r as u8);
            rgba.push(g as u8);
            rgba.push(b as u8);
            rgba.push(0xFF);
        }
    }

    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pipeline() -> (gst::Pipeline, gst_app::AppSink) {
        gst::init().unwrap();
        let pipeline = gst::parse::launch("videotestsrc ! video/x-raw,format=NV12,width=64,height=48,framerate=30/1 ! appsink name=output")
            .unwrap().downcast::<gst::Pipeline>().unwrap();
        let sink = pipeline
            .by_name("output")
            .unwrap()
            .downcast::<gst_app::AppSink>()
            .unwrap();
        (pipeline, sink)
    }

    #[test]
    fn blocked_native_seek_cannot_block_controls_or_stop() {
        let (pipeline, sink) = test_pipeline();
        let pad = sink.static_pad("sink").unwrap();
        let mut video = Video::from_gst_pipeline(pipeline.clone(), sink, None).unwrap();
        let (entered, waiting) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel::<()>();
        let resume = Mutex::new(resume);
        let probe = pad
            .add_probe(gst::PadProbeType::EVENT_UPSTREAM, move |_, info| {
                if info
                    .event()
                    .is_some_and(|event| event.type_() == gst::EventType::Seek)
                {
                    let _ = entered.send(());
                    // Fail-safe deadline keeps the test process self-terminating.
                    let _ = resume.lock().unwrap().recv_timeout(Duration::from_secs(3));
                }
                gst::PadProbeReturn::Ok
            })
            .unwrap();
        let start = Instant::now();
        video.seek(Duration::from_secs(1), false).unwrap();
        assert!(start.elapsed() < Duration::from_millis(100));
        waiting
            .recv_timeout(Duration::from_secs(2))
            .expect("seek did not reach native probe");
        let start = Instant::now();
        video.set_paused(true);
        video.set_volume(0.3);
        video.set_muted(true);
        let _ = video.position();
        assert!(video.paused());
        video.stop();
        assert!(start.elapsed() < Duration::from_millis(100));
        drop(release);
        pad.remove_probe(probe);
        assert!(video.shutdown(Duration::from_secs(2)));
        assert_eq!(pipeline.current_state(), gst::State::Null);
    }

    #[test]
    fn errors_are_received_while_paused_without_a_widget() {
        let (pipeline, sink) = test_pipeline();
        let video =
            Video::from_pipeline(pipeline.clone(), sink, None, &AtomicBool::new(false), true)
                .unwrap();
        pipeline
            .post_message(gst::message::Error::new(
                gst::StreamError::Decode,
                "injected decoder error",
            ))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(error) = video.take_error() {
                assert!(error.message().contains("injected decoder error"));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(video.shutdown(Duration::from_secs(2)));
    }

    #[test]
    fn blocked_stream_during_teardown_does_not_block_drop() {
        let (pipeline, sink) = test_pipeline();
        let pad = sink.static_pad("sink").unwrap();
        let mut video = Video::from_gst_pipeline(pipeline.clone(), sink, None).unwrap();
        let (entered, waiting) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel::<()>();
        let resume = Mutex::new(resume);
        let probe = pad
            .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                let _ = entered.send(());
                let _ = resume.lock().unwrap().recv_timeout(Duration::from_secs(3));
                gst::PadProbeReturn::Ok
            })
            .unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        // Retain join handles only in the test so we can prove the delayed
        // cleanup exits after releasing the simulated stuck native source.
        let workers = {
            let mut inner = video.get_mut();
            [
                inner.worker.take().unwrap(),
                inner.controller.take().unwrap(),
            ]
        };
        let start = Instant::now();
        drop(video);
        assert!(start.elapsed() < Duration::from_millis(100));
        drop(release);
        pad.remove_probe(probe);
        let deadline = Instant::now() + Duration::from_secs(2);
        for worker in workers {
            while !worker.is_finished() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            worker.join().unwrap();
        }
        assert_eq!(pipeline.current_state(), gst::State::Null);
    }

    #[test]
    fn loading_without_a_first_frame_can_be_cancelled() {
        gst::init().unwrap();
        let source = gst_app::AppSrc::builder().format(gst::Format::Time).build();
        let sink = gst_app::AppSink::builder().build();
        let pipeline = gst::Pipeline::new();
        pipeline
            .add_many([source.upcast_ref::<gst::Element>(), sink.upcast_ref()])
            .unwrap();
        source.link(&sink).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let worker_pipeline = pipeline.clone();
        let worker = std::thread::spawn(move || {
            Video::from_pipeline(worker_pipeline, sink, None, &token, true)
        });
        std::thread::sleep(Duration::from_millis(100));
        cancel.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
        assert_eq!(pipeline.current_state(), gst::State::Null);
    }

    #[test]
    fn odd_dimensions_and_padded_planes_are_copied_safely() {
        gst::init().unwrap();
        let info = VideoInfo::builder(gstreamer_video::VideoFormat::Nv12, 5, 3)
            .build()
            .unwrap();
        let mut data = vec![0; info.size()];
        for plane in 0..2 {
            let rows = if plane == 0 { 3 } else { 2 };
            let width = if plane == 0 { 5 } else { 6 };
            for row in 0..rows {
                let start = info.offset()[plane] + row * info.stride()[plane] as usize;
                data[start..start + width].fill(if plane == 0 { 20 } else { 150 });
            }
        }
        let sample = gst::Sample::builder()
            .caps(&info.to_caps().unwrap())
            .buffer(&gst::Buffer::from_mut_slice(data))
            .build();
        let frame = Frame::from_sample(&sample).unwrap();
        assert_eq!(frame.bytes.len(), 30);
        assert_eq!(&frame.bytes[0..6], &[20, 20, 20, 20, 20, 0]);
        assert_eq!(&frame.bytes[18..30], &[150; 12]);
        let short = gst::Sample::builder()
            .caps(&info.to_caps().unwrap())
            .buffer(&gst::Buffer::from_mut_slice(vec![0; 2]))
            .build();
        assert!(Frame::from_sample(&short).is_err());
    }

    fn assert_frames_delivered(framerate: Option<gst::Fraction>, has_duration: bool) {
        gst::init().unwrap();
        let mut caps = gst::Caps::builder("video/x-raw")
            .field("format", "NV12")
            .field("width", 4i32)
            .field("height", 4i32);
        if let Some(rate) = framerate {
            caps = caps.field("framerate", rate);
        }
        let source = gst_app::AppSrc::builder()
            .caps(&caps.build())
            .format(gst::Format::Time)
            .build();
        let sink = gst_app::AppSink::builder().sync(false).build();
        let pipeline = gst::Pipeline::new();
        pipeline
            .add_many([source.upcast_ref::<gst::Element>(), sink.upcast_ref()])
            .unwrap();
        source.link(&sink).unwrap();

        // Uneven timestamps model VFR. A valid decoded frame may have no duration.
        for (index, pts) in [0, 40, 160, 200].into_iter().enumerate() {
            let mut buffer = gst::Buffer::from_mut_slice(vec![index as u8; 24]);
            let metadata = buffer.get_mut().unwrap();
            metadata.set_pts(gst::ClockTime::from_mseconds(pts));
            if has_duration {
                metadata.set_duration(gst::ClockTime::from_mseconds(40));
            }
            source.push_buffer(buffer).unwrap();
        }
        source.end_of_stream().unwrap();

        let video = Video::from_gst_pipeline(pipeline.clone(), sink, None)
            .expect("valid timestamped video should load");
        assert_eq!(video.size(), (4, 4));
        assert_eq!(
            video.framerate(),
            framerate.map_or(0.0, |rate| { rate.numer() as f64 / rate.denom() as f64 })
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let delivered = {
                let inner = video.read();
                let frame = inner.frame.lock().unwrap();
                frame.pts == Some(gst::ClockTime::from_mseconds(200))
                    && frame.bytes.as_ref() == [3; 24]
            };
            if delivered {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "decoded frame never reached the renderer"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(video.shutdown(Duration::from_secs(2)));
        drop(video);
        assert_eq!(pipeline.current_state(), gst::State::Null);
    }

    #[test]
    fn variable_framerate_frames_reach_renderer() {
        assert_frames_delivered(Some(gst::Fraction::new(0, 1)), true);
    }

    #[test]
    fn variable_framerate_frames_without_duration_reach_renderer() {
        assert_frames_delivered(Some(gst::Fraction::new(0, 1)), false);
    }

    #[test]
    fn unspecified_framerate_frames_reach_renderer() {
        assert_frames_delivered(None, false);
    }

    #[test]
    fn fixed_framerate_frames_still_reach_renderer() {
        assert_frames_delivered(Some(gst::Fraction::new(30, 1)), true);
    }
}
