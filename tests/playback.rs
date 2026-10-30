#![cfg(feature = "gui")]
use gstreamer::{self as gst, prelude::*};
use iced_video_player::Video;
use std::{
    fs,
    os::windows::fs::OpenOptionsExt,
    path::Path,
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "playback did not make progress");
        thread::sleep(Duration::from_millis(10));
    }
}

fn fixture(path: &Path) {
    gst::init().unwrap();
    let pipeline = gst::parse::launch("videotestsrc num-buffers=120 pattern=smpte ! video/x-raw,width=160,height=90,framerate=30/1 ! videoconvert ! x264enc tune=zerolatency key-int-max=30 threads=2 ! h264parse ! mp4mux ! filesink name=output")
        .unwrap().downcast::<gst::Pipeline>().unwrap();
    pipeline
        .by_name("output")
        .unwrap()
        .set_property("location", path.to_str().unwrap());
    pipeline.set_state(gst::State::Playing).unwrap();
    let message = pipeline.bus().unwrap().timed_pop_filtered(
        gst::ClockTime::from_seconds(10),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    pipeline.set_state(gst::State::Null).unwrap();
    assert!(matches!(
        message.as_ref().map(|m| m.view()),
        Some(gst::MessageView::Eos(_))
    ));
}

fn exclusive(path: &Path) {
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .expect("player retained the file after shutdown");
}

#[test]
fn repeated_load_seek_pause_stop_releases_the_actual_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("播放 切换 #100%.mp4");
    fixture(&path);
    let uri = video_management::media::file_uri(&path).unwrap();
    for _ in 0..8 {
        let mut video = Video::new_cancellable(&uri, &AtomicBool::new(false), true).unwrap();
        let pipeline = video.pipeline();
        assert!(video.paused());
        assert_eq!(pipeline.current_state(), gst::State::Paused);
        video.set_volume(0.25);
        video.set_muted(true);
        video.set_paused(false);
        wait_for(|| video.position() > Duration::from_millis(50));
        wait_for(|| {
            pipeline.property::<bool>("mute")
                && (pipeline.property::<f64>("volume") - 0.25).abs() < 0.01
        });
        video.set_paused(true);
        wait_for(|| pipeline.current_state() == gst::State::Paused);
        // Coalesced seek commands must leave the final requested position.
        for milliseconds in [500, 1000, 1500, 2000] {
            video
                .seek(Duration::from_millis(milliseconds), true)
                .unwrap();
        }
        wait_for(|| video.position().abs_diff(Duration::from_secs(2)) < Duration::from_millis(100));
        assert!(video.take_error().is_none());
        assert!(video.shutdown(Duration::from_secs(3)));
        assert_eq!(pipeline.current_state(), gst::State::Null);
        drop(video);
        drop(pipeline);
        exclusive(&path);
    }
}

#[test]
fn end_of_stream_can_restart_without_a_widget() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.mp4");
    fixture(&path);
    let uri = video_management::media::file_uri(&path).unwrap();
    let mut video = Video::new(&uri).unwrap();
    video.seek(Duration::from_millis(3600), true).unwrap();
    wait_for(|| video.eos());
    video.set_paused(false);
    wait_for(|| {
        !video.eos()
            && video.position() > Duration::from_millis(50)
            && video.position() < Duration::from_secs(2)
    });
    assert!(video.take_error().is_none());
    assert!(video.shutdown(Duration::from_secs(3)));
    exclusive(&path);
}

#[test]
fn corrupt_missing_and_precancelled_files_fail_and_release_resources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupt.mp4");
    fs::write(&path, b"not a video").unwrap();
    let uri = video_management::media::file_uri(&path).unwrap();
    assert!(Video::new(&uri).is_err());
    exclusive(&path);
    fs::remove_file(&path).unwrap();
    assert!(Video::new(&uri).is_err());
    let start = Instant::now();
    assert!(matches!(
        Video::new_cancellable(&uri, &AtomicBool::new(true), true),
        Err(iced_video_player::Error::Cancelled)
    ));
    assert!(start.elapsed() < Duration::from_millis(100));
}
