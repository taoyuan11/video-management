#![cfg(feature = "gui")]

use gstreamer::{self as gst, prelude::*};
use std::{
    fs,
    os::windows::fs::OpenOptionsExt,
    path::Path,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
use video_management::{
    model::AppSettings,
    scanner::{self, ScanStats},
    thumbnails::{Outcome, Request, Source, Worker},
};

struct TestPipeline(gst::Pipeline);
impl Drop for TestPipeline {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

fn fixture(
    path: &Path,
    intro: bool,
    pattern: &str,
    dimensions: (u32, u32),
    rotate: bool,
    par: &str,
) {
    gst::init().unwrap();
    let caps = format!(
        "video/x-raw,format=I420,width={},height={},framerate=10/1,pixel-aspect-ratio={par}",
        dimensions.0, dimensions.1
    );
    let tags = if rotate {
        "taginject tags=\"image-orientation=rotate-90\" ! "
    } else {
        ""
    };
    let sources = if intro {
        format!(
            "videotestsrc pattern=black num-buffers=20 ! {caps} ! sequence.sink_0 videotestsrc pattern={pattern} num-buffers=20 ! {caps} ! sequence.sink_1"
        )
    } else {
        format!("videotestsrc pattern={pattern} num-buffers=20 ! {caps} ! sequence.sink_0")
    };
    let pipeline = TestPipeline(gst::parse::launch(&format!("concat name=sequence ! x264enc tune=zerolatency key-int-max=10 ! h264parse ! {tags}mp4mux ! filesink name=output {sources}"))
        .unwrap().downcast::<gst::Pipeline>().unwrap());
    pipeline
        .0
        .by_name("output")
        .unwrap()
        .set_property("location", path.to_str().unwrap());
    pipeline.0.set_state(gst::State::Playing).unwrap();
    let message = pipeline
        .0
        .bus()
        .unwrap()
        .timed_pop_filtered(
            gst::ClockTime::from_seconds(20),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        )
        .expect("fixture encoding timed out");
    if let gst::MessageView::Error(error) = message.view() {
        panic!("{} {:?}", error.error(), error.debug());
    }
}

fn request(path: &Path, id: u64) -> Request {
    let fp =
        scanner::fingerprint(path, true, &AtomicBool::new(false), &ScanStats::default()).unwrap();
    Request {
        id,
        hash: fp.hash,
        sources: vec![Source {
            path: fp.path,
            size: fp.size,
            modified: fp.modified,
        }],
    }
}

fn get(worker: &Worker, request: Request) -> Outcome {
    worker.request(vec![request.clone()]);
    let event = worker
        .results
        .recv_timeout(Duration::from_secs(12))
        .expect("cover worker timed out");
    assert_eq!(event.request, request);
    event.outcome
}

fn ready(outcome: Outcome) -> image::RgbaImage {
    match outcome {
        Outcome::Ready(image) => image,
        other => panic!("expected cover, got {other:?}"),
    }
}

fn exclusive(path: &Path) {
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .expect("decoder retained file handle");
}

#[test]
fn verbatim_long_path_supports_playback_and_cover_extraction() {
    let dir = tempfile::tempdir().unwrap();
    let mut folder = fs::canonicalize(dir.path()).unwrap();
    for _ in 0..8 {
        folder.push("长路径目录-用于验证播放组件保留Windows扩展路径前缀");
    }
    fs::create_dir_all(&folder).unwrap();
    let path = folder.join("中文 空格 #100% literal%20.mp4");
    assert!(path.to_str().unwrap().encode_utf16().count() > 260);
    fixture(&path, false, "smpte", (320, 180), false, "1/1");

    let uri = video_management::media::file_uri(&path).unwrap();
    let video = iced_video_player::Video::new(&uri).unwrap();
    assert_eq!(video.size(), (320, 180));
    drop(video);
    exclusive(&path);

    let pool = video_management::processing::Pool::new(AppSettings::default().concurrency).unwrap();

    let mut worker = Worker::start(dir.path().join("cache"), pool.executor()).unwrap();
    assert_eq!(
        ready(get(&worker, request(&path, 1))).dimensions(),
        (320, 180)
    );
    worker.shutdown();
    exclusive(&path);
}

#[test]
fn skips_solid_intro_reuses_and_repairs_cache_releases_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("中文片头样本.mp4");
    fixture(&path, true, "smpte", (640, 360), false, "1/1");
    let mut req = request(&path, 1);
    let cache = dir.path().join("cache");
    let pool = video_management::processing::Pool::new(AppSettings::default().concurrency).unwrap();
    let mut worker = Worker::start(cache.clone(), pool.executor()).unwrap();
    let image = ready(get(&worker, req.clone()));
    assert_eq!(image.dimensions(), (320, 180));
    assert!(image.pixels().any(|p| p[0] > 100 && p[1] < 80));
    assert!(image.pixels().all(|p| p[3] == 255));
    exclusive(&path);
    req.id += 1;
    let saved = req.sources.clone();
    req.sources.clear();
    assert_eq!(
        ready(get(&worker, req.clone())),
        image,
        "cached image should work offline"
    );
    let png = fs::read_dir(&cache)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(png, b"corrupted png").unwrap();
    req.id += 1;
    req.sources = saved;
    assert_eq!(
        ready(get(&worker, req)),
        image,
        "corrupt cache should regenerate"
    );
    worker.shutdown();
    exclusive(&path);
}

#[test]
fn handles_solid_corrupt_missing_and_unwritable_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("纯色.mp4");
    fixture(&path, false, "red", (320, 180), false, "1/1");
    let pool = video_management::processing::Pool::new(AppSettings::default().concurrency).unwrap();
    let mut worker = Worker::start(dir.path().join("cache"), pool.executor()).unwrap();
    assert!(matches!(get(&worker, request(&path, 1)), Outcome::NoFrame));
    exclusive(&path);
    fs::write(&path, b"not a movie").unwrap();
    assert!(matches!(
        get(&worker, request(&path, 2)),
        Outcome::Failed(_)
    ));
    let missing = request(&path, 3);
    fs::remove_file(&path).unwrap();
    assert!(matches!(get(&worker, missing), Outcome::Failed(_)));
    worker.shutdown();
    fixture(&path, false, "smpte", (320, 180), false, "1/1");
    let blocked = dir.path().join("blocked");
    fs::write(&blocked, b"cache directory cannot be created").unwrap();
    let pool = video_management::processing::Pool::new(AppSettings::default().concurrency).unwrap();
    let mut worker = Worker::start(blocked, pool.executor()).unwrap();
    ready(get(&worker, request(&path, 4)));
    worker.shutdown();
    exclusive(&path);
}

#[test]
fn respects_portrait_rotation_and_non_square_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let pool = video_management::processing::Pool::new(AppSettings::default().concurrency).unwrap();
    let mut worker = Worker::start(dir.path().join("cache"), pool.executor()).unwrap();
    for (id, size, rotate, par, expected) in [
        (1, (360, 640), false, "1/1", (180, 320)),
        (2, (640, 360), true, "1/1", (180, 320)),
        (3, (320, 240), false, "2/1", (320, 120)),
    ] {
        let path = dir.path().join(format!("{id}.mp4"));
        fixture(&path, false, "smpte", size, rotate, par);
        assert_eq!(
            ready(get(&worker, request(&path, id))).dimensions(),
            expected
        );
        exclusive(&path);
    }
    worker.shutdown();
}

#[test]
fn changed_content_new_generation_and_cancelled_queue_do_not_mix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replace.mp4");
    fixture(&path, false, "smpte", (320, 180), false, "1/1");
    let old = request(&path, 1);
    let pool = video_management::processing::Pool::new(AppSettings::default().concurrency).unwrap();
    let mut worker = Worker::start(dir.path().join("cache"), pool.executor()).unwrap();
    let first = ready(get(&worker, old.clone()));
    fixture(&path, false, "checkers-8", (320, 180), false, "1/1");
    let current = request(&path, 2);
    assert_ne!(old.hash, current.hash);
    let second = ready(get(&worker, current.clone()));
    assert_ne!(first, second);
    // Rapid demand replacement: only matching request identities may be displayed.
    for id in 3..30 {
        worker.request(vec![Request {
            id,
            ..current.clone()
        }]);
        worker.request(vec![]);
    }
    worker.request(vec![Request {
        id: 30,
        ..current.clone()
    }]);
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let event = worker
            .results
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        if event.request.id == 30 {
            assert_eq!(event.request.hash, current.hash);
            assert_eq!(ready(event.outcome), second);
            break;
        }
    }
    let started = Instant::now();
    worker.shutdown();
    assert!(started.elapsed() < Duration::from_secs(2));
    exclusive(&path);
}
