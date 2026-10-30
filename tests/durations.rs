use std::path::Path;
use video_management::{database::Database, model::*};

fn observe(db: &mut Database, path: &Path, hash: &str) {
    db.observe(&Fingerprint {
        path: path.into(),
        size: 100,
        modified: 1,
        hash: hash.into(),
        explicit: true,
    })
    .unwrap();
}

#[test]
fn version_two_migrates_and_durations_follow_content_across_restarts_and_replacements() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    let source = dir.path().join("视频.mp4");
    let copy = dir.path().join("副本.mp4");
    let hash = "a".repeat(64);
    let mut db = Database::open(&path).unwrap();
    observe(&mut db, &source, &hash);
    observe(&mut db, &copy, &hash);
    let id = db.snapshot().unwrap().videos[0].id;
    db.create_tag("收藏").unwrap();
    let tag = db.snapshot().unwrap().tags[0].id;
    db.assign(&[id], TagAssignment::Add(vec![tag]), true)
        .unwrap();
    drop(db);
    // Recreate the previous released schema, including existing user records.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "ALTER TABLE videos DROP COLUMN duration_ms;
        ALTER TABLE videos DROP COLUMN duration_checked; PRAGMA user_version=2;",
    )
    .unwrap();
    drop(conn);

    let db = Database::open(&path).unwrap();
    let video = &db.snapshot().unwrap().videos[0];
    assert_eq!(video.duration_ms, None);
    assert_eq!(video.tag_ids, [tag]);
    assert_eq!(video.files.len(), 2);
    assert!(!db.duration_status(false).unwrap()[&id].checked);
    db.set_duration(&hash, Some(3_600_123)).unwrap();
    db.set_duration(&hash, None).unwrap();
    db.availability(&source, Availability::Missing).unwrap();
    db.availability(&copy, Availability::Missing).unwrap();
    drop(db);

    let mut db = Database::open(&path).unwrap();
    let video = &db.snapshot().unwrap().videos[0];
    assert_eq!(video.duration_ms, Some(3_600_123));
    assert!(video.missing());
    assert!(db.duration_status(true).unwrap()[&id].checked);
    let replacement = "b".repeat(64);
    observe(&mut db, &source, &replacement);
    db.set_duration(&hash, Some(2_000)).unwrap();
    let snapshot = db.snapshot().unwrap();
    let new = snapshot
        .videos
        .iter()
        .find(|v| v.hash == replacement)
        .unwrap();
    assert_eq!(new.duration_ms, None);
    db.set_duration(&replacement, None).unwrap();
    assert!(db.duration_status(false).unwrap()[&new.id].checked);
    assert!(!db.duration_status(true).unwrap()[&new.id].checked);
    db.set_duration(&replacement, Some(500)).unwrap();
    assert!(db.duration_status(true).unwrap()[&new.id].checked);
}

#[cfg(feature = "gui")]
mod media_tests {
    use super::*;
    use gstreamer::{self as gst, prelude::*};
    use std::{
        fs,
        os::windows::fs::OpenOptionsExt,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, SystemTime},
    };
    use video_management::{
        media,
        processing::Pool,
        scanner::{self, ScanEvent, ScanRequest, ScanStats},
    };

    struct Pipeline(gst::Pipeline);
    impl Drop for Pipeline {
        fn drop(&mut self) {
            let _ = self.0.set_state(gst::State::Null);
        }
    }

    fn fixture(path: &Path, webm: bool) {
        let encoding = if webm {
            "vp8enc deadline=1 ! webmmux"
        } else {
            "x264enc tune=zerolatency ! h264parse ! mp4mux"
        };
        encode(
            path,
            &format!(
                "videotestsrc num-buffers=20 ! video/x-raw,width=160,height=90,framerate=10/1 ! {encoding} ! filesink name=output"
            ),
        );
    }

    fn encode(path: &Path, description: &str) {
        gst::init().unwrap();
        let pipeline = Pipeline(
            gst::parse::launch(description)
                .unwrap()
                .downcast::<gst::Pipeline>()
                .unwrap(),
        );
        pipeline
            .0
            .by_name("output")
            .unwrap()
            .set_property("location", path.to_str().unwrap());
        pipeline.0.set_state(gst::State::Playing).unwrap();
        let result = pipeline
            .0
            .bus()
            .unwrap()
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(15),
                &[gst::MessageType::Error, gst::MessageType::Eos],
            )
            .unwrap();
        assert!(
            matches!(result.view(), gst::MessageView::Eos(_)),
            "fixture failed: {result:?}"
        );
        drop(pipeline);
        filetime::set_file_mtime(
            path,
            filetime::FileTime::from_system_time(SystemTime::now() - Duration::from_secs(10)),
        )
        .unwrap();
    }

    fn exclusive(path: &Path) {
        fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .expect("metadata reader retained a file handle");
    }

    fn scan(db: &mut Database, full: bool, explicit: Vec<std::path::PathBuf>) -> (usize, u64) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let pool = Pool::new(1).unwrap();
        let stats = Arc::new(ScanStats::default());
        scanner::scan(
            ScanRequest {
                roots: vec![],
                known: db.files().unwrap(),
                explicit,
                full,
                durations: db.duration_status(full).unwrap(),
            },
            tx,
            Arc::new(AtomicBool::new(false)),
            stats.clone(),
            pool.executor(),
        );
        let mut durations = 0;
        for event in rx.try_iter() {
            match event {
                ScanEvent::Found(found) => {
                    db.observe(&found).unwrap();
                }
                ScanEvent::Duration(hash, millis) => {
                    db.set_duration(&hash, millis).unwrap();
                    durations += 1;
                }
                ScanEvent::Availability(path, availability) => {
                    db.availability(&path, availability).unwrap()
                }
                _ => {}
            }
        }
        (durations, stats.bytes.load(Ordering::Relaxed))
    }

    #[test]
    fn container_duration_handles_mp4_webm_unicode_failure_and_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        for (name, webm) in [("中文 #100% '测试'.mp4", false), ("测试.webm", true)] {
            let path = dir.path().join(name);
            fixture(&path, webm);
            let millis = media::duration_ms(&path, &AtomicBool::new(false))
                .unwrap()
                .unwrap();
            assert!((1_950..=2_050).contains(&millis), "duration: {millis}");
            exclusive(&path);
            assert!(media::duration_ms(&path, &AtomicBool::new(true)).is_err());
            exclusive(&path);
        }
        let corrupt = dir.path().join("损坏.mp4");
        fs::write(&corrupt, b"invalid container").unwrap();
        assert!(media::duration_ms(&corrupt, &AtomicBool::new(false)).is_err());
        exclusive(&corrupt);
        assert!(
            media::duration_ms(&dir.path().join("missing.mp4"), &AtomicBool::new(false)).is_err()
        );
    }

    #[test]
    fn multitrack_container_duration_includes_the_longer_audio_stream() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("音视频.mp4");
        encode(&path, "mp4mux name=mux ! filesink name=output
            videotestsrc num-buffers=20 ! video/x-raw,width=160,height=90,framerate=10/1 ! x264enc tune=zerolatency ! h264parse ! mux.video_0
            audiotestsrc num-buffers=30 samplesperbuffer=4800 ! audio/x-raw,rate=48000 ! avenc_aac ! aacparse ! mux.audio_0");
        let millis = media::duration_ms(&path, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(
            (3_000..=3_100).contains(&millis),
            "container duration: {millis}"
        );
        exclusive(&path);
    }

    #[test]
    fn scan_backfills_without_rehashing_and_caches_imports_and_failed_probes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("旧视频.mp4");
        fixture(&path, false);
        let mut db = Database::open(&dir.path().join("library.sqlite3")).unwrap();
        let found =
            scanner::fingerprint(&path, true, &AtomicBool::new(false), &ScanStats::default())
                .unwrap();
        db.observe(&found).unwrap();
        assert_eq!(scan(&mut db, false, vec![]), (1, 0));
        assert_eq!(db.snapshot().unwrap().videos[0].duration_ms, Some(2_000));
        assert_eq!(scan(&mut db, false, vec![]), (0, 0));
        exclusive(&path);
        let copy = dir.path().join("副本.mp4");
        fs::copy(&path, &copy).unwrap();
        assert_eq!(scan(&mut db, false, vec![copy]).0, 0);
        assert_eq!(db.snapshot().unwrap().videos[0].files.len(), 2);

        let imported = dir.path().join("新视频.webm");
        fixture(&imported, true);
        assert_eq!(scan(&mut db, false, vec![imported.clone()]).0, 1);
        assert!(
            db.snapshot()
                .unwrap()
                .videos
                .iter()
                .all(|v| v.duration_ms == Some(2_000))
        );
        exclusive(&imported);

        let invalid = dir.path().join("损坏.mp4");
        fs::write(&invalid, b"invalid").unwrap();
        assert_eq!(scan(&mut db, false, vec![invalid.clone()]).0, 1);
        assert_eq!(scan(&mut db, false, vec![]), (0, 0));
        assert_eq!(scan(&mut db, true, vec![]).0, 1);
        exclusive(&invalid);
    }

    #[test]
    fn locked_backfill_source_is_retried_when_access_returns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("视频.mp4");
        fixture(&path, false);
        let mut db = Database::open(&dir.path().join("library.sqlite3")).unwrap();
        let found =
            scanner::fingerprint(&path, true, &AtomicBool::new(false), &ScanStats::default())
                .unwrap();
        db.observe(&found).unwrap();
        let writer = fs::OpenOptions::new()
            .write(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        assert_eq!(scan(&mut db, false, vec![]), (0, 0));
        drop(writer);
        assert_eq!(scan(&mut db, false, vec![]), (1, 0));
        exclusive(&path);
    }
}
