//! Usage: cargo run --example player_smoke -- path/to/generated-fixture.mp4
use gstreamer::prelude::*;
use iced_video_player::Video;
use std::{os::windows::fs::OpenOptionsExt, path::PathBuf, time::Duration};

fn main() -> anyhow::Result<()> {
    gstreamer::init()?;
    for plugin in gstreamer::Registry::get().plugins() {
        anyhow::ensure!(
            !plugin
                .plugin_flags()
                .contains(gstreamer::PluginFlags::BLACKLISTED),
            "Blacklisted runtime plugin: {}",
            plugin.name()
        );
    }
    for factory in [
        "playbin",
        "appsink",
        "filesrc",
        "qtdemux",
        "matroskademux",
        "avidemux",
        "asfdemux",
        "flvdemux",
        "mpegpsdemux",
        "tsdemux",
        "avdec_h264",
        "avdec_h265",
        "avdec_mpeg2video",
        "avdec_wmv2",
        "avdec_aac",
        "vp8dec",
        "vp9dec",
        "dav1ddec",
        "opusdec",
        "vorbisdec",
        "videoflip",
        "wasapi2sink",
    ] {
        anyhow::ensure!(
            gstreamer::ElementFactory::find(factory).is_some(),
            "Missing {factory}"
        );
    }
    let path = dunce::canonicalize(PathBuf::from(
        std::env::args_os().nth(1).expect("fixture path"),
    ))?;
    let uri = video_management::media::file_uri(&path)?;
    let mut video = Video::new(&uri)?;
    let pipeline = video.pipeline();
    let sink = pipeline
        .property::<gstreamer::Element>("video-sink")
        .downcast::<gstreamer::Bin>()
        .unwrap()
        .by_name("iced_video")
        .unwrap()
        .downcast::<gstreamer_app::AppSink>()
        .unwrap();
    assert_eq!(sink.max_buffers(), 2);
    assert!(sink.is_drop());
    assert!(!sink.property::<bool>("enable-last-sample"));
    assert!(!sink.is_wait_on_eos());
    drop(sink);
    drop(pipeline);
    assert!(video.size().0 > 0 && video.size().1 > 0);
    assert!(video.duration() > Duration::from_secs(2));
    std::thread::sleep(Duration::from_millis(300));
    assert!(video.position() > Duration::ZERO);
    video.set_paused(true);
    std::thread::sleep(Duration::from_millis(100));
    let position = video.position();
    std::thread::sleep(Duration::from_millis(150));
    assert!(video.position().abs_diff(position) < Duration::from_millis(100));
    video.seek(Duration::from_secs(2), true)?;
    std::thread::sleep(Duration::from_millis(200));
    assert!(video.position().abs_diff(Duration::from_secs(2)) < Duration::from_millis(300));
    video.set_volume(0.35);
    assert!((video.volume() - 0.35).abs() < 0.01);
    video.set_muted(true);
    assert!(video.muted());
    let streams: i32 = video.pipeline().property("n-audio");
    println!(
        "OK: {:?}, {}x{}, duration={:?}, audio_streams={streams}, pause/seek/volume/mute",
        path.file_name(),
        video.size().0,
        video.size().1,
        video.duration()
    );
    assert!(
        video.shutdown(Duration::from_secs(3)),
        "player teardown timed out"
    );
    drop(video);
    let pool = video_management::processing::Pool::new(
        2.min(video_management::model::AppSettings::max_concurrency()),
    )?;
    let cache = tempfile::tempdir()?;
    let fingerprint = video_management::scanner::fingerprint(
        &path,
        true,
        &std::sync::atomic::AtomicBool::new(false),
        &video_management::scanner::ScanStats::default(),
    )?;
    let mut worker =
        video_management::thumbnails::Worker::start(cache.path().into(), pool.executor())?;
    worker.request(vec![video_management::thumbnails::Request {
        id: 1,
        hash: fingerprint.hash,
        sources: vec![video_management::thumbnails::Source {
            path: path.clone(),
            size: fingerprint.size,
            modified: fingerprint.modified,
        }],
    }]);
    let result = worker.results.recv_timeout(Duration::from_secs(12))?;
    anyhow::ensure!(
        matches!(
            result.outcome,
            video_management::thumbnails::Outcome::Ready(_)
        ),
        "Cover extraction failed: {:?}",
        result.outcome
    );
    worker.shutdown();
    drop(pool);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    options.share_mode(0);
    let file = options.open(&path)?;
    drop(file);
    println!("OK: player released file and worker");
    Ok(())
}
