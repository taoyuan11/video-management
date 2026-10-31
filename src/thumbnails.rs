//! Bounded, silent cover extraction. File and decoder work uses the application executor.
use crate::processing::{Executor, Lane};
use crossbeam_channel::{Receiver, SendTimeoutError, Sender, bounded};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::AppSink;
use gstreamer_video::{VideoFrameRef, VideoInfo, prelude::*};
use image::{ImageFormat, RgbaImage};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const VERSION: &str = "v1";
const MAX_EDGE: u32 = 320;
// A decoder can retain many full-resolution reference frames, independently
// of the small output image. Keep cover decoding bounded on high-core CPUs.
const MAX_ACTIVE: usize = 2;
const RESULT_CAPACITY: usize = 4;
const BUDGET: Duration = Duration::from_secs(10);
const POLL: gst::ClockTime = gst::ClockTime::from_mseconds(50);
const SOLID_STD_DEV: f64 = 6.0;
const SOLID_TOLERANCE: u8 = 12;
const SOLID_PERCENT: usize = 98;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub size: u64,
    pub modified: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Unique per attempt; prevents cancelled or replaced requests from updating new state.
    pub id: u64,
    pub hash: String,
    pub sources: Vec<Source>,
}

#[derive(Debug)]
pub enum Outcome {
    Ready(RgbaImage),
    NoFrame,
    Failed(String),
    Cancelled,
}

pub struct ResultEvent {
    pub request: Request,
    pub outcome: Outcome,
}

#[derive(Default)]
struct Queue {
    desired: Vec<Request>,
    finished: HashSet<u64>,
    active: HashMap<u64, Active>,
    stopped: bool,
}
struct Active {
    hash: String,
    cancel: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
}
type SharedQueue = Arc<(Mutex<Queue>, Condvar)>;

// Also runs when an executor drops queued work, so cancellation cannot strand a request.
struct Completion {
    queue: SharedQueue,
    id: u64,
    cancel: Arc<AtomicBool>,
}
impl Drop for Completion {
    fn drop(&mut self) {
        let mut q = self.queue.0.lock().unwrap();
        q.active.remove(&self.id);
        if !self.cancel.load(Ordering::Relaxed) && q.desired.iter().any(|r| r.id == self.id) {
            q.finished.insert(self.id);
        }
        self.queue.1.notify_all();
    }
}

pub struct Worker {
    queue: SharedQueue,
    executor: Executor,
    pub results: Receiver<ResultEvent>,
    join: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn start(cache: PathBuf, executor: Executor) -> std::io::Result<Self> {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let shared = queue.clone();
        let dispatch = executor.clone();
        let (tx, results) = bounded(RESULT_CAPACITY);
        let join = thread::Builder::new()
            .name("video-cover-queue".into())
            .spawn(move || {
                loop {
                    let (request, cancel, started) = {
                        let mut q = shared.0.lock().unwrap();
                        loop {
                            if q.stopped {
                                return;
                            }
                            if q.active.len() < MAX_ACTIVE
                                && let Some(request) = q
                                    .desired
                                    .iter()
                                    .find(|r| {
                                        !q.finished.contains(&r.id)
                                            && !q.active.contains_key(&r.id)
                                            && !q.active.values().any(|a| a.hash == r.hash)
                                    })
                                    .cloned()
                            {
                                let cancel = Arc::new(AtomicBool::new(false));
                                let started = Arc::new(AtomicBool::new(false));
                                q.active.insert(
                                    request.id,
                                    Active {
                                        hash: request.hash.clone(),
                                        cancel: cancel.clone(),
                                        started: started.clone(),
                                    },
                                );
                                break (request, cancel, started);
                            }
                            q = shared.1.wait(q).unwrap();
                        }
                    };
                    let completion = Completion {
                        queue: shared.clone(),
                        id: request.id,
                        cancel: cancel.clone(),
                    };
                    let (cache, tx) = (cache.clone(), tx.clone());
                    if !dispatch.submit(Lane::Cover, cancel.clone(), move || {
                        let _completion = completion;
                        started.store(true, Ordering::Relaxed);
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                generate(&cache, &request, &cancel)
                            }))
                            .unwrap_or_else(|_| Outcome::Failed("封面生成异常".into()));
                        send_result(&tx, ResultEvent { request, outcome }, &cancel);
                    }) {
                        return;
                    }
                }
            })?;
        Ok(Self {
            queue,
            executor,
            results,
            join: Some(join),
        })
    }
    /// Replace demand in priority order, cancelling obsolete or not-yet-started work.
    pub fn request(&self, desired: Vec<Request>) {
        {
            let mut q = self.queue.0.lock().unwrap();
            if q.desired == desired || q.stopped {
                return;
            }
            let ids: HashSet<_> = desired.iter().map(|r| r.id).collect();
            for (id, active) in &q.active {
                if !ids.contains(id) || !active.started.load(Ordering::Relaxed) {
                    active.cancel.store(true, Ordering::Relaxed);
                }
            }
            q.finished.retain(|id| ids.contains(id));
            q.desired = desired;
            self.queue.1.notify_all();
        }
        self.executor.discard_cancelled();
    }
    pub fn shutdown(&mut self) {
        {
            let mut q = self.queue.0.lock().unwrap();
            q.stopped = true;
            q.desired.clear();
            for active in q.active.values() {
                active.cancel.store(true, Ordering::Relaxed);
            }
            self.queue.1.notify_all();
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        self.executor.discard_cancelled();
        let mut q = self.queue.0.lock().unwrap();
        while !q.active.is_empty() {
            q = self.queue.1.wait(q).unwrap();
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn send_result(tx: &Sender<ResultEvent>, mut event: ResultEvent, cancel: &AtomicBool) {
    // The UI may stop draining during shutdown. Never block cancellation on a
    // full result queue, or retain obsolete decoded images while changing pages.
    while !cancel.load(Ordering::Relaxed) {
        match tx.send_timeout(event, Duration::from_millis(50)) {
            Ok(()) | Err(SendTimeoutError::Disconnected(_)) => return,
            Err(SendTimeoutError::Timeout(pending)) => event = pending,
        }
    }
}

fn cache_path(cache: &Path, hash: &str) -> Option<PathBuf> {
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| cache.join(format!("{VERSION}-{hash}.png")))
}

fn read_cache(path: &Path) -> Option<RgbaImage> {
    if fs::metadata(path).ok()?.len() > 1024 * 1024 {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_EDGE);
    limits.max_image_height = Some(MAX_EDGE);
    limits.max_alloc = Some(2 * 1024 * 1024);
    reader.limits(limits);
    let mut image = reader.decode().ok()?.into_rgba8();
    for pixel in image.pixels_mut() {
        pixel[3] = 255;
    }
    (!near_solid(&image)).then_some(image)
}

fn write_cache(path: &Path, image: &RgbaImage) -> anyhow::Result<()> {
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    image.write_to(&mut file, ImageFormat::Png)?;
    file.flush()?;
    file.persist(path)?;
    Ok(())
}

fn source_matches(source: &Source) -> bool {
    fs::metadata(&source.path).is_ok_and(|meta| {
        meta.is_file()
            && meta.len() == source.size
            && crate::scanner::modified(&meta) == source.modified
    })
}

fn generate(cache: &Path, request: &Request, cancel: &AtomicBool) -> Outcome {
    let deadline = Instant::now() + BUDGET;
    if cancel.load(Ordering::Relaxed) {
        return Outcome::Cancelled;
    }
    let Some(path) = cache_path(cache, &request.hash) else {
        return Outcome::Failed("无效的内容指纹".into());
    };
    if let Some(image) = read_cache(&path) {
        return if cancel.load(Ordering::Relaxed) {
            Outcome::Cancelled
        } else {
            Outcome::Ready(image)
        };
    }
    let mut last_error = "文件不可访问".to_string();
    for source in &request.sources {
        if cancel.load(Ordering::Relaxed) {
            return Outcome::Cancelled;
        }
        if Instant::now() >= deadline {
            return Outcome::Failed("封面生成超时".into());
        }
        if !source_matches(source) {
            continue;
        }
        match extract(&source.path, deadline, cancel) {
            Ok(Some(image)) if source_matches(source) => {
                if cancel.load(Ordering::Relaxed) {
                    return Outcome::Cancelled;
                }
                // Caching is opportunistic; a read-only/full disk must not hide a valid frame.
                let _ = write_cache(&path, &image);
                return Outcome::Ready(image);
            }
            Ok(Some(_)) => last_error = "文件内容已变化".into(),
            Ok(None) if source_matches(source) => return Outcome::NoFrame,
            Ok(None) => last_error = "文件内容已变化".into(),
            Err(e) => last_error = format!("{e:#}"),
        }
    }
    if cancel.load(Ordering::Relaxed) {
        Outcome::Cancelled
    } else {
        Outcome::Failed(last_error)
    }
}

struct Pipeline(gst::Pipeline);
impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

fn extract(
    path: &Path,
    deadline: Instant,
    cancel: &AtomicBool,
) -> anyhow::Result<Option<RgbaImage>> {
    gst::init()?;
    // Static pipeline text only; the file URI is assigned as a property (Unicode/quotes safe).
    // No audio/text flags or real-time playback clock; each candidate is a paused preroll.
    let pipeline = Pipeline(gst::parse::launch(
        "playbin flags=video video-sink=\"videoconvert ! videoflip video-direction=auto ! videoscale add-borders=false ! video/x-raw,format=RGBA,width=[1,320],height=[1,320],pixel-aspect-ratio=1/1 ! appsink name=cover sync=false max-buffers=1 drop=true wait-on-eos=false enable-last-sample=false\""
    )?.downcast::<gst::Pipeline>().map_err(|_| anyhow::anyhow!("无法创建取帧管线"))?);
    let uri = crate::media::file_uri(path)?;
    pipeline.0.set_property("uri", uri.as_str());
    // libav's automatic frame threading scales its reference-frame allocation
    // with CPU count. A cover needs a few paused samples, not playback throughput.
    pipeline.0.connect_deep_element_added(|_, _, element| {
        if element
            .factory()
            .is_some_and(|factory| factory.name().starts_with("avdec_"))
            && element.has_property_with_type("max-threads", i32::static_type())
        {
            element.set_property("max-threads", 1i32);
        }
    });
    let sink = pipeline
        .0
        .property::<gst::Element>("video-sink")
        .downcast::<gst::Bin>()
        .map_err(|_| anyhow::anyhow!("无法创建取帧组件"))?
        .by_name("cover")
        .unwrap()
        .downcast::<AppSink>()
        .unwrap();
    pipeline.0.set_state(gst::State::Paused)?;
    let first = pull_frame(&pipeline.0, &sink, deadline, cancel)?;
    if !near_solid(&first) {
        return Ok(Some(first));
    }
    let duration = pipeline
        .0
        .query_duration::<gst::ClockTime>()
        .map(|t| t.nseconds());
    for nanos in candidates(duration).into_iter().skip(1) {
        check_budget(deadline, cancel)?;
        pipeline.0.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::ClockTime::from_nseconds(nanos),
        )?;
        let frame = pull_frame(&pipeline.0, &sink, deadline, cancel)?;
        if !near_solid(&frame) {
            return Ok(Some(frame));
        }
    }
    Ok(None)
}

fn check_budget(deadline: Instant, cancel: &AtomicBool) -> anyhow::Result<()> {
    anyhow::ensure!(!cancel.load(Ordering::Relaxed), "封面生成已取消");
    anyhow::ensure!(Instant::now() < deadline, "封面生成超时");
    Ok(())
}

fn pull_frame(
    pipeline: &gst::Pipeline,
    sink: &AppSink,
    deadline: Instant,
    cancel: &AtomicBool,
) -> anyhow::Result<RgbaImage> {
    let bus = pipeline.bus().unwrap();
    loop {
        check_budget(deadline, cancel)?;
        for message in bus.iter() {
            if let gst::MessageView::Error(error) = message.view() {
                anyhow::bail!("{}", error.error());
            }
        }
        if let Some(sample) = sink.try_pull_preroll(POLL) {
            let info = VideoInfo::from_caps(
                sample
                    .caps()
                    .ok_or_else(|| anyhow::anyhow!("画面格式缺失"))?,
            )?;
            let frame = VideoFrameRef::from_buffer_ref_readable(
                sample
                    .buffer()
                    .ok_or_else(|| anyhow::anyhow!("画面数据缺失"))?,
                &info,
            )?;
            let data = frame.plane_data(0)?;
            let stride = usize::try_from(frame.plane_stride()[0])?;
            anyhow::ensure!(
                info.width() > 0
                    && info.height() > 0
                    && info.width() <= MAX_EDGE
                    && info.height() <= MAX_EDGE,
                "画面尺寸无效"
            );
            let row_len = info.width() as usize * 4;
            anyhow::ensure!(
                stride >= row_len && data.len() >= stride * (info.height() as usize - 1) + row_len,
                "画面行跨度无效"
            );
            let mut image = RgbaImage::new(info.width(), info.height());
            for (y, row) in image.as_mut().chunks_exact_mut(row_len).enumerate() {
                row.copy_from_slice(&data[y * stride..y * stride + row_len]);
                for pixel in row.chunks_exact_mut(4) {
                    pixel[3] = 255;
                }
            }
            return Ok(image);
        }
        anyhow::ensure!(!sink.is_eos(), "没有可解码的视频画面");
    }
}

fn candidates(duration: Option<u64>) -> Vec<u64> {
    let mut times: Vec<_> = [0, 250, 500, 1000, 2000, 4000, 8000, 16000, 30000]
        .into_iter()
        .map(|ms| ms * 1_000_000)
        .collect();
    if let Some(duration) = duration.filter(|d| *d > 0) {
        times.extend([10, 25, 50, 75, 90].map(|percent| duration / 100 * percent));
        times.retain(|time| *time < duration);
    }
    times.sort_unstable();
    times.dedup();
    times
}

fn near_solid(image: &RgbaImage) -> bool {
    if image.width() == 0 || image.height() == 0 {
        return true;
    }
    let small = image::imageops::thumbnail(image, 64, 64);
    let count = (small.width() * small.height()) as usize;
    let mut histogram = [[0usize; 256]; 3];
    let mut sum = [0.0; 3];
    let mut squares = [0.0; 3];
    for pixel in small.pixels() {
        for c in 0..3 {
            histogram[c][pixel[c] as usize] += 1;
            sum[c] += f64::from(pixel[c]);
            squares[c] += f64::from(pixel[c]).powi(2);
        }
    }
    if (0..3).all(|c| {
        squares[c] / count as f64 - (sum[c] / count as f64).powi(2) <= SOLID_STD_DEV.powi(2)
    }) {
        return true;
    }
    let medians = histogram.map(|channel| {
        let mut n = 0;
        channel
            .iter()
            .position(|v| {
                n += v;
                n > count / 2
            })
            .unwrap() as u8
    });
    let similar = small
        .pixels()
        .filter(|pixel| (0..3).all(|c| pixel[c].abs_diff(medians[c]) <= SOLID_TOLERANCE))
        .count();
    similar * 100 >= count * SOLID_PERCENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_result_queue_bounds_work_and_shutdown_does_not_need_a_consumer() {
        let dir = tempfile::tempdir().unwrap();
        let image = RgbaImage::from_fn(320, 180, |x, y| image::Rgba([x as u8, y as u8, 100, 255]));
        let requests: Vec<_> = (1..=20)
            .map(|id| {
                let hash = format!("{id:064x}");
                write_cache(&cache_path(dir.path(), &hash).unwrap(), &image).unwrap();
                Request {
                    id,
                    hash,
                    sources: vec![],
                }
            })
            .collect();
        let mut pool =
            crate::processing::Pool::new(crate::model::AppSettings::max_concurrency().min(4))
                .unwrap();
        let mut worker = Worker::start(dir.path().into(), pool.executor()).unwrap();
        worker.request(requests);
        let deadline = Instant::now() + Duration::from_secs(5);
        while worker.results.len() < RESULT_CAPACITY {
            assert!(
                Instant::now() < deadline,
                "cache jobs did not fill result queue"
            );
            thread::sleep(Duration::from_millis(10));
        }
        // Give producers time to hit backpressure rather than relying on a
        // single instantaneous observation of a still-growing unbounded queue.
        thread::sleep(Duration::from_millis(150));
        assert_eq!(worker.results.len(), RESULT_CAPACITY);
        {
            let queue = worker.queue.0.lock().unwrap();
            assert!(queue.active.len() <= MAX_ACTIVE);
            assert!(queue.finished.len() <= RESULT_CAPACITY);
        }
        let start = Instant::now();
        worker.shutdown();
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(worker.queue.0.lock().unwrap().active.is_empty());
        pool.shutdown();
    }

    #[test]
    fn solids_noise_and_real_dark_color_content() {
        for color in [[0, 0, 0, 255], [255, 255, 255, 255], [210, 40, 100, 255]] {
            assert!(near_solid(&RgbaImage::from_pixel(
                80,
                50,
                image::Rgba(color)
            )));
        }
        let noise = RgbaImage::from_fn(64, 64, |x, y| {
            image::Rgba([((x * 17 + y * 13) % 9) as u8; 4])
        });
        assert!(near_solid(&noise));
        let sparse_noise = RgbaImage::from_fn(64, 64, |x, y| {
            image::Rgba(if x == 0 && y < 32 {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            })
        });
        assert!(near_solid(&sparse_noise));
        let dark = RgbaImage::from_fn(64, 64, |x, _| {
            image::Rgba(if x < 32 {
                [2, 3, 4, 255]
            } else {
                [30, 32, 35, 255]
            })
        });
        assert!(!near_solid(&dark));
        let chroma = RgbaImage::from_fn(64, 64, |x, _| {
            image::Rgba(if x < 32 {
                [255, 0, 0, 255]
            } else {
                [0, 76, 0, 255]
            })
        });
        assert!(!near_solid(&chroma));
    }

    #[test]
    fn timestamps_are_ordered_unique_and_inside_short_clips() {
        for duration in [
            1,
            100_000_000,
            1_000_000_000,
            5_000_000_000,
            600_000_000_000,
        ] {
            let times = candidates(Some(duration));
            assert_eq!(times[0], 0);
            assert!(times.windows(2).all(|w| w[0] < w[1]));
            assert!(times.iter().all(|t| *t < duration));
        }
    }

    #[test]
    fn cache_is_bounded_validated_and_versioned() {
        let dir = tempfile::tempdir().unwrap();
        assert!(cache_path(dir.path(), "../wrong").is_none());
        let path = cache_path(dir.path(), &"a".repeat(64)).unwrap();
        fs::write(&path, b"broken").unwrap();
        assert!(read_cache(&path).is_none());
        let image = RgbaImage::from_fn(320, 180, |x, y| image::Rgba([x as u8, y as u8, 100, 255]));
        write_cache(&path, &image).unwrap();
        assert_eq!(read_cache(&path).unwrap(), image);
    }
}
