use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant, SystemTime},
};
use tempfile::TempDir;
use video_management::{
    database::Database,
    model::*,
    scanner::{self, ScanEvent, ScanRequest, ScanStats},
    service::{Command, Event, Service},
};

fn file(root: &Path, name: &str, content: &[u8]) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    filetime::set_file_mtime(
        &path,
        filetime::FileTime::from_system_time(SystemTime::now() - Duration::from_secs(10)),
    )
    .unwrap();
    scanner::normalize(&path).unwrap()
}
fn observe(db: &mut Database, path: &Path, explicit: bool) -> bool {
    let f = scanner::fingerprint(
        path,
        explicit,
        &AtomicBool::new(false),
        &ScanStats::default(),
    )
    .unwrap();
    db.observe(&f).unwrap()
}
fn scan(db: &mut Database, roots: Vec<PathBuf>, full: bool) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let pool = video_management::processing::Pool::new(AppSettings::default().concurrency).unwrap();
    scanner::scan(
        ScanRequest {
            roots,
            known: db.files().unwrap(),
            durations: db.duration_status(full).unwrap(),
            explicit: vec![],
            full,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        Arc::new(ScanStats::default()),
        pool.executor(),
    );
    for e in rx.try_iter() {
        match e {
            ScanEvent::Found(f) => {
                db.observe(&f).unwrap();
            }
            ScanEvent::Availability(p, a) => db.availability(&p, a).unwrap(),
            ScanEvent::Duration(hash, millis) => db.set_duration(&hash, millis).unwrap(),
            _ => {}
        }
    }
}
fn setup() -> (TempDir, Database) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(&tmp.path().join("test.sqlite3")).unwrap();
    (tmp, db)
}

#[test]
fn identical_bytes_merge_but_same_names_do_not() {
    let (tmp, mut db) = setup();
    let a = file(tmp.path(), "中文/视频.mp4", b"abc");
    let b = file(tmp.path(), "副本.MP4", b"abc");
    let c = file(tmp.path(), "不同/视频.mp4", b"abd");
    assert!(observe(&mut db, &a, false));
    assert!(observe(&mut db, &b, false));
    assert!(observe(&mut db, &c, false));
    assert!(!observe(&mut db, &a, false));
    let s = db.snapshot().unwrap();
    assert_eq!(s.videos.len(), 2);
    assert_eq!(
        s.videos.iter().find(|v| v.files.len() == 2).unwrap().hash,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn tagging_persists_without_touching_files() {
    let (tmp, mut db) = setup();
    let p = file(tmp.path(), "旅行.mp4", b"travel");
    observe(&mut db, &p, true);
    let id = db.snapshot().unwrap().videos[0].id;
    db.create_tag("旅行").unwrap();
    let gid = db.snapshot().unwrap().tags[0].id;
    db.assign(&[id], TagAssignment::Add(vec![gid]), true)
        .unwrap();
    db.rename_tag(gid, "旅途").unwrap();
    drop(db);
    let db = Database::open(&tmp.path().join("test.sqlite3")).unwrap();
    let s = db.snapshot().unwrap();
    assert!(s.videos[0].library);
    assert_eq!(s.videos[0].tag_ids, vec![gid]);
    assert_eq!(s.tags[0].name, "旅途");
    assert_eq!(fs::read(&p).unwrap(), b"travel");
    db.delete_tag(gid).unwrap();
    assert_eq!(db.snapshot().unwrap().videos[0].tag_ids, Vec::<i64>::new());
    assert!(p.exists());
}

#[test]
fn ignore_and_forget_do_not_repeat_until_reimport_or_content_change() {
    let (tmp, mut db) = setup();
    let p = file(tmp.path(), "忽略.mp4", b"first");
    observe(&mut db, &p, false);
    let id = db.snapshot().unwrap().videos[0].id;
    db.forget(&[id], true).unwrap();
    assert!(!observe(&mut db, &p, false));
    assert!(db.snapshot().unwrap().videos.is_empty());
    assert!(p.exists());
    assert!(observe(&mut db, &p, true));
    db.assign(&[id], TagAssignment::Keep, true).unwrap();
    db.forget(&[id], false).unwrap();
    assert!(!observe(&mut db, &p, false));
    assert!(db.snapshot().unwrap().videos.is_empty());
    fs::write(&p, b"changed").unwrap();
    assert!(observe(&mut db, &p, false));
    assert_eq!(db.snapshot().unwrap().videos.len(), 1);
}

#[test]
fn rename_relinks_and_new_copy_waits_for_confirmation() {
    let (tmp, mut db) = setup();
    let dir = tmp.path().join("videos");
    let a = file(&dir, "old.mp4", b"identity");
    observe(&mut db, &a, false);
    let id = db.snapshot().unwrap().videos[0].id;
    db.create_tag("收藏").unwrap();
    let gid = db.snapshot().unwrap().tags[0].id;
    db.create_tag("旅行").unwrap();
    let other = db
        .snapshot()
        .unwrap()
        .tags
        .iter()
        .find(|t| t.id != gid)
        .unwrap()
        .id;
    let mut tag_ids = vec![gid, other];
    tag_ids.sort_unstable();
    db.assign(&[id], TagAssignment::Add(tag_ids.clone()), true)
        .unwrap();
    let renamed = dir.join("renamed.mp4");
    fs::rename(&a, &renamed).unwrap();
    scan(&mut db, vec![dir.clone()], false);
    let s = db.snapshot().unwrap();
    assert_eq!(s.videos.len(), 1);
    assert!(!s.videos[0].pending());
    assert!(!s.videos[0].missing());
    assert_eq!(s.videos[0].tag_ids, tag_ids);
    file(&dir, "copy.mp4", b"identity");
    scan(&mut db, vec![dir], false);
    let s = db.snapshot().unwrap();
    assert_eq!(s.videos.len(), 1);
    assert!(s.videos[0].pending());
    assert_eq!(s.videos[0].tag_ids, tag_ids);
}

#[test]
fn replaced_path_becomes_pending_and_old_content_stays_missing() {
    let (tmp, mut db) = setup();
    let p = file(tmp.path(), "same.mp4", b"old");
    observe(&mut db, &p, false);
    let id = db.snapshot().unwrap().videos[0].id;
    db.assign(&[id], TagAssignment::Keep, true).unwrap();
    fs::write(&p, b"new").unwrap();
    observe(&mut db, &p, false);
    let s = db.snapshot().unwrap();
    assert_eq!(s.videos.len(), 2);
    assert!(s.videos.iter().find(|v| v.id == id).unwrap().missing());
    assert!(s.videos.iter().find(|v| v.id != id).unwrap().pending());
}

#[test]
fn full_verification_detects_same_size_and_timestamp_replacement() {
    let (tmp, mut db) = setup();
    let dir = tmp.path().join("videos");
    let p = file(&dir, "same.mp4", b"old");
    observe(&mut db, &p, false);
    let hash = db.snapshot().unwrap().videos[0].hash.clone();
    let stamp = filetime::FileTime::from_last_modification_time(&fs::metadata(&p).unwrap());
    fs::write(&p, b"new").unwrap();
    filetime::set_file_mtime(&p, stamp).unwrap();
    scan(&mut db, vec![dir.clone()], false);
    assert_eq!(db.snapshot().unwrap().videos[0].hash, hash);
    scan(&mut db, vec![dir], true);
    assert_ne!(db.snapshot().unwrap().videos[0].hash, hash);
}

#[test]
fn overlapping_roots_are_deduplicated_and_nonvideos_are_skipped() {
    let (tmp, mut db) = setup();
    let root = tmp.path().join("videos");
    let sub = root.join("sub");
    file(&sub, "one.mp4", b"v");
    file(&sub, "ignore.txt", b"x");
    scan(&mut db, vec![root, sub], false);
    assert_eq!(db.files().unwrap().len(), 1);
}

#[test]
fn offline_root_keeps_managed_records() {
    let (tmp, mut db) = setup();
    let root = tmp.path().join("drive");
    let p = file(&root, "one.mp4", b"v");
    observe(&mut db, &p, false);
    let id = db.snapshot().unwrap().videos[0].id;
    db.assign(&[id], TagAssignment::Keep, true).unwrap();
    fs::rename(&root, tmp.path().join("offline")).unwrap();
    scan(&mut db, vec![root], false);
    let s = db.snapshot().unwrap();
    assert!(s.videos[0].missing());
    assert_eq!(
        s.videos[0].files[0].availability,
        Availability::Inaccessible
    );
}

#[test]
fn deleting_one_location_preserves_other_copy_and_tag() {
    let (tmp, mut db) = setup();
    let a = file(tmp.path(), "a.mp4", b"v");
    let b = file(tmp.path(), "b.mp4", b"v");
    observe(&mut db, &a, false);
    observe(&mut db, &b, false);
    let id = db.snapshot().unwrap().videos[0].id;
    db.create_tag("keep").unwrap();
    let gid = db.snapshot().unwrap().tags[0].id;
    db.assign(&[id], TagAssignment::Add(vec![gid]), true)
        .unwrap();
    let first = db.files().unwrap()[0].id;
    db.remove_file(first).unwrap();
    let s = db.snapshot().unwrap();
    assert_eq!(s.videos[0].files.len(), 1);
    assert_eq!(s.videos[0].tag_ids, vec![gid]);
    let last = s.videos[0].files[0].id;
    db.remove_file(last).unwrap();
    assert!(db.snapshot().unwrap().videos.is_empty());
}

#[test]
fn invalid_tag_assignment_rolls_back_and_names_are_validated() {
    let (tmp, mut db) = setup();
    let p = file(tmp.path(), "a.mp4", b"v");
    observe(&mut db, &p, false);
    let id = db.snapshot().unwrap().videos[0].id;
    assert!(db.create_tag(" ").is_err());
    db.create_tag("Videos").unwrap();
    assert!(db.create_tag("videos").is_err());
    assert!(
        db.assign(&[id], TagAssignment::Add(vec![999]), true)
            .is_err()
    );
    assert!(!db.snapshot().unwrap().videos[0].library);
}

#[test]
fn cancelled_hash_never_produces_identity() {
    let (tmp, _) = setup();
    let p = file(tmp.path(), "large.mp4", &vec![42; 2 * 1024 * 1024]);
    let cancel = AtomicBool::new(true);
    assert!(scanner::fingerprint(&p, false, &cancel, &ScanStats::default()).is_err());
}

#[test]
fn file_open_for_writing_is_deferred() {
    use std::os::windows::fs::OpenOptionsExt;
    let (tmp, _) = setup();
    let p = file(tmp.path(), "copying.mp4", b"copy");
    let writer = fs::OpenOptions::new()
        .write(true)
        .share_mode(3)
        .open(&p)
        .unwrap();
    assert!(
        scanner::fingerprint(&p, false, &AtomicBool::new(false), &ScanStats::default()).is_err()
    );
    drop(writer);
    assert!(
        scanner::fingerprint(&p, false, &AtomicBool::new(false), &ScanStats::default()).is_ok()
    );
}

fn wait_snapshot(service: &Service, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        match service.events.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Snapshot(s)) if predicate(&s) => return s,
            Ok(Event::Error(e)) => panic!("{e}"),
            _ => {}
        }
    }
    panic!("Timed out waiting for snapshot")
}

fn wait_deletion(service: &Service) -> (Vec<DeleteProgress>, DeleteOutcome, Snapshot) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut progress = Vec::new();
    let mut snapshot = None;
    while Instant::now() < deadline {
        match service.events.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::DeleteProgress(value)) => progress.push(value),
            Ok(Event::Snapshot(value)) => snapshot = Some(value),
            Ok(Event::DeleteFinished(outcome)) => {
                return (
                    progress,
                    outcome,
                    snapshot.expect("records published before completion"),
                );
            }
            Ok(Event::Error(error)) => panic!("deletion must report its own outcome: {error}"),
            _ => {}
        }
    }
    panic!("Timed out waiting for deletion outcome")
}

#[test]
fn watcher_detects_new_file_and_shutdown_joins_workers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("videos");
    fs::create_dir(&root).unwrap();
    let mut service = Service::start(tmp.path().join("data")).unwrap();
    service.send(Command::AddRoots(vec![root.clone()]));
    wait_snapshot(&service, |s| !s.roots.is_empty());
    file(&root, "new.mp4", b"created after startup");
    wait_snapshot(&service, |s| s.videos.iter().any(VideoItem::pending));
    service.send(Command::Scan(true));
    let before = Instant::now();
    service.shutdown();
    assert!(before.elapsed() < Duration::from_secs(5));
    assert!(
        service
            .events
            .try_iter()
            .any(|e| matches!(e, Event::Stopped))
    );
}

#[test]
fn restart_finds_files_added_while_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("videos");
    fs::create_dir(&root).unwrap();
    let data = tmp.path().join("data");
    let mut service = Service::start(data.clone()).unwrap();
    service.send(Command::AddRoots(vec![root.clone()]));
    wait_snapshot(&service, |s| !s.roots.is_empty());
    service.shutdown();
    file(&root, "offline.mp4", b"offline");
    let mut service = Service::start(data).unwrap();
    wait_snapshot(&service, |s| s.videos.iter().any(VideoItem::pending));
    service.shutdown();
}

#[test]
fn permanent_deletion_preserves_unselected_copy_and_removes_last_record() {
    let tmp = tempfile::tempdir().unwrap();
    let a = file(tmp.path(), "delete-test-only.mp4", b"test fixture");
    let b = file(tmp.path(), "keep-test-only.mp4", b"test fixture");
    let mut service = Service::start(tmp.path().join("data")).unwrap();
    service.send(Command::Import(vec![a.clone(), b.clone()]));
    let s = wait_snapshot(&service, |s| {
        s.videos.first().is_some_and(|v| v.files.len() == 2)
    });
    let v = &s.videos[0];
    let target = v.files.iter().find(|f| f.path == a).unwrap();
    service.send(Command::Assign(vec![v.id], TagAssignment::Keep, true));
    wait_snapshot(&service, |s| s.videos[0].library);
    service.send(Command::Delete(
        v.id,
        vec![DeleteTarget {
            file_id: target.id,
            path: a.clone(),
            hash: v.hash.clone(),
        }],
    ));
    let (progress, outcome, snapshot) = wait_deletion(&service);
    assert_eq!(outcome.total, 1);
    assert_eq!(outcome.deleted, 1);
    assert!(outcome.failures.is_empty());
    assert_eq!(snapshot.videos[0].files.len(), 1);
    assert_eq!(progress.first().unwrap().stage, DeleteStage::Preparing);
    assert!(
        progress
            .iter()
            .any(|p| p.stage == DeleteStage::WaitingForScan)
    );
    for stage in [DeleteStage::Verifying, DeleteStage::Removing] {
        let step = progress.iter().find(|p| p.stage == stage).unwrap();
        assert_eq!(step.path.as_ref(), Some(&a));
        assert_eq!((step.completed, step.total), (0, 1));
    }
    assert!(!a.exists());
    assert!(b.exists());
    let kept = v.files.iter().find(|f| f.path == b).unwrap();
    service.send(Command::Delete(
        v.id,
        vec![DeleteTarget {
            file_id: kept.id,
            path: b.clone(),
            hash: v.hash.clone(),
        }],
    ));
    let (_, outcome, snapshot) = wait_deletion(&service);
    assert_eq!(outcome.deleted, 1);
    assert!(outcome.failures.is_empty());
    assert!(snapshot.videos.is_empty());
    service.shutdown();
    assert!(!b.exists());
}

#[test]
fn deletion_rejects_a_request_containing_multiple_videos_before_deleting_any_file() {
    let tmp = tempfile::tempdir().unwrap();
    let a = file(tmp.path(), "first.mp4", b"first video");
    let b = file(tmp.path(), "second.mp4", b"second video");
    let data = tmp.path().join("data");
    let mut service = Service::start(data.clone()).unwrap();
    service.send(Command::Import(vec![a.clone(), b.clone()]));
    let snapshot = wait_snapshot(&service, |s| s.videos.len() == 2);
    let targets = snapshot
        .videos
        .iter()
        .map(|v| DeleteTarget {
            file_id: v.files[0].id,
            path: v.files[0].path.clone(),
            hash: v.hash.clone(),
        })
        .collect();
    service.send(Command::Delete(snapshot.videos[0].id, targets));
    let (progress, outcome, _) = wait_deletion(&service);
    service.shutdown();
    assert_eq!(
        (outcome.total, outcome.deleted, outcome.failures.len()),
        (2, 0, 2)
    );
    assert!(
        outcome
            .failures
            .iter()
            .all(|failure| failure.reason.contains("其他视频"))
    );
    assert!(progress.iter().all(|p| p.stage == DeleteStage::Preparing));
    assert!(a.exists());
    assert!(b.exists());
    let db = Database::open(&data.join("library.sqlite3")).unwrap();
    assert_eq!(db.files().unwrap().len(), 2);
}

#[test]
fn forget_command_changes_only_one_video_and_preserves_original_files() {
    for pending in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let a = file(tmp.path(), "first.mp4", b"first video");
        let b = file(tmp.path(), "second.mp4", b"second video");
        let mut service = Service::start(tmp.path().join("data")).unwrap();
        service.send(Command::Import(vec![a.clone(), b.clone()]));
        let snapshot = wait_snapshot(&service, |s| s.videos.len() == 2);
        let target = snapshot.videos[0].id;
        if !pending {
            service.send(Command::Assign(
                snapshot.videos.iter().map(|v| v.id).collect(),
                TagAssignment::Keep,
                true,
            ));
            wait_snapshot(&service, |s| s.videos.iter().all(|v| v.library));
        }
        service.send(Command::Forget(target, pending));
        let result = wait_snapshot(&service, |s| {
            s.videos.len() == 1 && s.videos[0].id != target
        });
        service.shutdown();
        let other = result.videos.iter().find(|v| v.id != target).unwrap();
        assert_eq!(other.pending(), pending);
        assert_eq!(other.library, !pending);
        assert!(a.exists());
        assert!(b.exists());
    }
}

#[test]
fn changed_file_is_not_deleted_using_stale_confirmation() {
    let tmp = tempfile::tempdir().unwrap();
    let path = file(tmp.path(), "changing.mp4", b"original");
    let mut service = Service::start(tmp.path().join("data")).unwrap();
    service.send(Command::Import(vec![path.clone()]));
    let s = wait_snapshot(&service, |s| !s.videos.is_empty());
    let v = &s.videos[0];
    let f = &v.files[0];
    fs::write(&path, b"replacement").unwrap();
    service.send(Command::Delete(
        v.id,
        vec![DeleteTarget {
            file_id: f.id,
            path: path.clone(),
            hash: v.hash.clone(),
        }],
    ));
    let (_, outcome, _) = wait_deletion(&service);
    service.shutdown();
    assert_eq!((outcome.deleted, outcome.failures.len()), (0, 1));
    assert_eq!(outcome.failures[0].path, path);
    assert!(outcome.failures[0].reason.contains("未删除"));
    assert!(outcome.failures[0].reason.contains("变化"));
    assert_eq!(fs::read(&path).unwrap(), b"replacement");
}

#[test]
fn locked_file_deletion_keeps_database_record() {
    use std::os::windows::fs::OpenOptionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let path = file(tmp.path(), "locked.mp4", b"locked");
    let mut service = Service::start(tmp.path().join("data")).unwrap();
    service.send(Command::Import(vec![path.clone()]));
    let s = wait_snapshot(&service, |s| !s.videos.is_empty());
    let v = &s.videos[0];
    let f = &v.files[0];
    let writer = fs::OpenOptions::new()
        .write(true)
        .share_mode(3)
        .open(&path)
        .unwrap();
    service.send(Command::Delete(
        v.id,
        vec![DeleteTarget {
            file_id: f.id,
            path: path.clone(),
            hash: v.hash.clone(),
        }],
    ));
    let (_, outcome, _) = wait_deletion(&service);
    service.shutdown();
    drop(writer);
    assert_eq!((outcome.deleted, outcome.failures.len()), (0, 1));
    assert_eq!(outcome.failures[0].path, path);
    assert!(outcome.failures[0].reason.contains("占用"));
    assert!(path.exists());
    let db = Database::open(&tmp.path().join("data/library.sqlite3")).unwrap();
    assert_eq!(db.files().unwrap().len(), 1);
}

#[test]
fn deletion_reports_each_failure_and_continues_with_other_confirmed_copies() {
    use std::os::windows::fs::OpenOptionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let paths: Vec<_> = ["locked.mp4", "changed.mp4", "delete.mp4", "keep.mp4"]
        .iter()
        .map(|name| file(tmp.path(), name, b"same video"))
        .collect();
    let mut service = Service::start(tmp.path().join("data")).unwrap();
    service.send(Command::Import(paths.clone()));
    let snapshot = wait_snapshot(&service, |s| {
        s.videos.first().is_some_and(|v| v.files.len() == 4)
    });
    let video = &snapshot.videos[0];
    // Allow verification reads but deny deletion, covering failure after hashing.
    let reader = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&paths[0])
        .unwrap();
    fs::write(&paths[1], b"changed content").unwrap();
    let targets = paths[..3]
        .iter()
        .map(|path| DeleteTarget {
            file_id: video
                .files
                .iter()
                .find(|file| &file.path == path)
                .unwrap()
                .id,
            path: path.clone(),
            hash: video.hash.clone(),
        })
        .collect();
    service.send(Command::Delete(video.id, targets));
    let (progress, outcome, snapshot) = wait_deletion(&service);
    service.shutdown();
    drop(reader);
    assert_eq!(
        (outcome.total, outcome.deleted, outcome.failures.len()),
        (3, 1, 2)
    );
    assert_eq!(outcome.failures[0].path, paths[0]);
    assert!(outcome.failures[0].reason.contains("无法删除文件"));
    assert!(outcome.failures[0].reason.contains("占用"));
    assert_eq!(outcome.failures[1].path, paths[1]);
    assert!(outcome.failures[1].reason.contains("内容已变化"));
    let verified: Vec<_> = progress
        .iter()
        .filter(|p| p.stage == DeleteStage::Verifying)
        .map(|p| p.completed)
        .collect();
    assert_eq!(verified, vec![0, 1, 2]);
    assert_eq!(snapshot.videos[0].files.len(), 3);
    assert!(paths[0].exists());
    assert_eq!(fs::read(&paths[1]).unwrap(), b"changed content");
    assert!(!paths[2].exists());
    assert!(paths[3].exists());
}

#[test]
fn deletion_reports_the_actual_result_for_missing_and_readonly_files() {
    for readonly in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let path = file(tmp.path(), "fixture.mp4", b"test fixture");
        let mut service = Service::start(tmp.path().join("data")).unwrap();
        service.send(Command::Import(vec![path.clone()]));
        let snapshot = wait_snapshot(&service, |s| !s.videos.is_empty());
        let video = &snapshot.videos[0];
        let permissions = fs::metadata(&path).unwrap().permissions();
        if readonly {
            let mut locked = permissions.clone();
            locked.set_readonly(true);
            fs::set_permissions(&path, locked).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        service.send(Command::Delete(
            video.id,
            vec![DeleteTarget {
                file_id: video.files[0].id,
                path: path.clone(),
                hash: video.hash.clone(),
            }],
        ));
        let (_, outcome, _) = wait_deletion(&service);
        service.shutdown();
        let still_exists = path.exists();
        if readonly && still_exists {
            fs::set_permissions(&path, permissions).unwrap();
        }
        // Windows can permit deleting a read-only file. Its attribute alone
        // must not be reported as a permission failure after a successful delete.
        if readonly && !still_exists {
            assert_eq!((outcome.total, outcome.deleted), (1, 1));
            assert!(outcome.failures.is_empty());
            continue;
        }
        assert_eq!(
            (outcome.total, outcome.deleted, outcome.failures.len()),
            (1, 0, 1)
        );
        assert_eq!(outcome.failures[0].path, path);
        assert!(outcome.failures[0].reason.contains(if readonly {
            "访问被拒绝"
        } else {
            "检查路径"
        }));
    }
}

#[test]
fn empty_deletion_request_reports_failure_and_service_remains_available() {
    let tmp = tempfile::tempdir().unwrap();
    let mut service = Service::start(tmp.path().join("data")).unwrap();
    wait_snapshot(&service, |_| true);
    service.send(Command::Delete(1, vec![]));
    let (_, outcome, _) = wait_deletion(&service);
    assert_eq!(outcome.deleted, 0);
    assert_eq!(outcome.failures.len(), 1);
    assert!(outcome.failures[0].reason.contains("未选择文件"));
    service.send(Command::CreateTag("仍可使用".into()));
    wait_snapshot(&service, |s| s.tags.len() == 1);
    service.shutdown();
}

#[test]
fn database_can_only_be_managed_by_one_running_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let mut first = Service::start(tmp.path().to_path_buf()).unwrap();
    wait_snapshot(&first, |_| true);
    let mut second = Service::start(tmp.path().to_path_buf()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut rejected = false;
    while Instant::now() < deadline {
        if let Ok(Event::Error(error)) = second.events.recv_timeout(Duration::from_millis(200)) {
            rejected = error.contains("另一个");
            break;
        }
    }
    second.shutdown();
    first.shutdown();
    assert!(rejected);
    let mut reopened = Service::start(tmp.path().to_path_buf()).unwrap();
    wait_snapshot(&reopened, |_| true);
    reopened.shutdown();
}

#[test]
fn explicitly_imported_busy_file_is_retained_for_retry() {
    use std::os::windows::fs::OpenOptionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let path = file(tmp.path(), "busy.mp4", b"waiting");
    let writer = fs::OpenOptions::new()
        .write(true)
        .share_mode(3)
        .open(&path)
        .unwrap();
    let (tx, rx) = crossbeam_channel::unbounded();
    let pool = video_management::processing::Pool::new(AppSettings::default().concurrency).unwrap();
    scanner::scan(
        ScanRequest {
            roots: vec![],
            known: vec![],
            explicit: vec![path.clone()],
            full: false,
            durations: Default::default(),
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        Arc::new(ScanStats::default()),
        pool.executor(),
    );
    let mut retry = Vec::new();
    for e in rx.try_iter() {
        if let ScanEvent::Retry(p) = e {
            retry.push(p);
        }
    }
    assert_eq!(retry, vec![path]);
    drop(writer);
    let (tx, rx) = crossbeam_channel::unbounded();
    let pool = video_management::processing::Pool::new(AppSettings::default().concurrency).unwrap();
    scanner::scan(
        ScanRequest {
            roots: vec![],
            known: vec![],
            explicit: retry,
            full: false,
            durations: Default::default(),
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        Arc::new(ScanStats::default()),
        pool.executor(),
    );
    assert!(rx.try_iter().any(|e| matches!(e, ScanEvent::Found(_))));
}
