use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};
use video_management::{
    database::Database,
    migration::{self, Outcome, Request, Worker},
    model::*,
    scanner::{self, ScanStats},
};

fn sample(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    scanner::normalize(&path).unwrap()
}
fn observe(db: &mut Database, path: &Path) -> i64 {
    let f =
        scanner::fingerprint(path, false, &AtomicBool::new(false), &ScanStats::default()).unwrap();
    db.observe(&f).unwrap();
    db.files()
        .unwrap()
        .iter()
        .find(|f| f.path == path)
        .unwrap()
        .video_id
}
fn drive(
    db: &mut Database,
    mut worker: Worker,
    mut hook: impl FnMut(&Request, &Worker) -> bool,
) -> Outcome {
    loop {
        let request = worker.events.recv_timeout(Duration::from_secs(20)).unwrap();
        if hook(&request, &worker) {
            continue;
        }
        if let Request::Finished(outcome) = request {
            worker.finish();
            return outcome;
        }
        migration::answer(db, request);
    }
}
fn run(db: &mut Database, source: &Path, parent: &Path, copy: bool) -> Outcome {
    drive(
        db,
        Worker::start(source.into(), parent.into(), vec![], copy).unwrap(),
        |_, _| false,
    )
}

#[test]
fn complete_tree_preserves_identity_states_tags_and_overlapping_roots() {
    for copy in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut db = Database::open(&temp.path().join("library.sqlite3")).unwrap();
        let source = temp.path().join("中文视频");
        let parent = temp.path().join("归档");
        fs::create_dir(&parent).unwrap();
        let managed = sample(&source, "子目录/影片.mp4", b"managed");
        let pending = sample(&source, "待处理.mkv", b"pending");
        let ignored = sample(&source, "忽略.mov", b"ignored");
        sample(&source, "子目录/字幕.srt", b"subtitle");
        sample(&source, "封面.jpg", b"cover");
        fs::create_dir_all(source.join("空目录/子目录")).unwrap();
        let managed_id = observe(&mut db, &managed);
        observe(&mut db, &pending);
        let ignored_id = observe(&mut db, &ignored);
        let missing = sample(&source, "缺失.mp4", b"missing");
        let missing_id = observe(&mut db, &missing);
        db.assign(&[missing_id], TagAssignment::Keep, true).unwrap();
        fs::remove_file(&missing).unwrap();
        db.availability(&missing, Availability::Missing).unwrap();
        db.create_tag("旅行").unwrap();
        let tag = db.snapshot().unwrap().tags[0].id;
        db.assign(&[managed_id], TagAssignment::Add(vec![tag]), true)
            .unwrap();
        db.forget(&[ignored_id], true).unwrap();
        let external = sample(temp.path(), "外部副本.mp4", b"managed");
        observe(&mut db, &external);
        let hash = db
            .snapshot()
            .unwrap()
            .videos
            .iter()
            .find(|v| v.id == managed_id)
            .unwrap()
            .hash
            .clone();
        db.set_duration(&hash, Some(1234)).unwrap();
        db.add_root(&source).unwrap();
        db.add_root(&source.join("子目录")).unwrap();
        let before = db.files().unwrap();
        let outcome = run(&mut db, &source, &parent, copy);
        assert!(!outcome.retry, "{}", outcome.message);
        assert!(!source.exists(), "{}", outcome.message);
        let target = parent.join("中文视频");
        assert!(target.join("空目录/子目录").is_dir());
        assert_eq!(
            fs::read(target.join("子目录/字幕.srt")).unwrap(),
            b"subtitle"
        );
        assert_eq!(fs::read(target.join("封面.jpg")).unwrap(), b"cover");
        let after = db.files().unwrap();
        for old in &before {
            let new = after.iter().find(|f| f.id == old.id).unwrap();
            assert_eq!(new.video_id, old.video_id);
            assert_eq!(new.state, old.state);
            assert_eq!(new.availability, old.availability);
            assert_eq!(
                new.path,
                migration::relative_under(&old.path, &source)
                    .map(|r| target.join(r))
                    .unwrap_or_else(|| old.path.clone())
            );
            if new.availability == Availability::Present {
                let f = scanner::fingerprint(
                    &new.path,
                    false,
                    &AtomicBool::new(false),
                    &ScanStats::default(),
                )
                .unwrap();
                assert_eq!(
                    f.modified, old.modified,
                    "copy must preserve the scan cache timestamp"
                );
                assert!(
                    !db.observe(&f).unwrap(),
                    "migration must not create pending notifications"
                );
            }
        }
        let snapshot = db.snapshot().unwrap();
        let video = snapshot.videos.iter().find(|v| v.id == managed_id).unwrap();
        assert_eq!(video.tag_ids, [tag]);
        assert_eq!(video.duration_ms, Some(1234));
        assert_eq!(snapshot.roots, [target.clone(), target.join("子目录")]);
        assert!(db.migration_journal().unwrap().is_none());
    }
}

#[test]
fn invalid_destinations_and_database_collisions_preserve_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("videos");
    let parent = temp.path().join("target");
    sample(&source, "one.mp4", b"one");
    fs::create_dir(&parent).unwrap();
    assert!(migration::destination(&source, temp.path(), &[]).is_err());
    assert!(migration::destination(&source, &source, &[]).is_err());
    assert!(migration::destination(&source, &parent, std::slice::from_ref(&source)).is_err());
    fs::create_dir(parent.join("videos")).unwrap();
    assert!(migration::destination(&source, &parent, &[]).is_err());
    fs::remove_dir(parent.join("videos")).unwrap();
    let mut db = Database::open(&temp.path().join("db")).unwrap();
    db.add_root(&source).unwrap();
    db.observe(&Fingerprint {
        path: parent.join("videos/old.mp4"),
        size: 1,
        modified: 0,
        hash: "a".repeat(64),
        explicit: false,
    })
    .unwrap();
    assert!(
        db.validate_migration(&source, &parent.join("videos"))
            .is_err()
    );
    let outcome = run(&mut db, &source, &parent, false);
    assert!(
        outcome.message.contains("目标路径已有视频记录"),
        "{}",
        outcome.message
    );
    assert_eq!(fs::read(source.join("one.mp4")).unwrap(), b"one");
    assert!(!parent.join("videos").exists());
}

#[test]
fn cancellation_and_commit_failure_roll_back_both_move_strategies() {
    for copy in [false, true] {
        for cancel in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("videos");
            let parent = temp.path().join("target");
            let path = sample(&source, "one.mp4", &vec![17; 2 * 1024 * 1024]);
            fs::create_dir(&parent).unwrap();
            let mut db = Database::open(&temp.path().join("db")).unwrap();
            db.add_root(&source).unwrap();
            observe(&mut db, &path);
            let worker = Worker::start(source.clone(), parent.clone(), vec![], copy).unwrap();
            let outcome = drive(&mut db, worker, |request, worker| {
                if cancel && matches!(request, Request::Progress(p) if p.cancellable) {
                    worker.cancel();
                }
                if !cancel && let Request::Commit(_, reply) = request {
                    reply.send(Err("injected database failure".into())).unwrap();
                    return true;
                }
                false
            });
            assert!(!outcome.retry, "{}", outcome.message);
            assert!(source.join("one.mp4").exists());
            assert!(fs::read_dir(&parent).unwrap().next().is_none());
            assert_eq!(db.files().unwrap()[0].path, path);
            assert!(db.migration_journal().unwrap().is_none());
        }
    }
}

#[test]
fn modified_source_after_commit_is_preserved_and_cleanup_can_be_retried() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("videos");
    let parent = temp.path().join("target");
    sample(&source, "one.mp4", b"one");
    fs::create_dir(&parent).unwrap();
    let mut db = Database::open(&temp.path().join("db")).unwrap();
    db.add_root(&source).unwrap();
    let outcome = drive(
        &mut db,
        Worker::start(source.clone(), parent.clone(), vec![], true).unwrap(),
        |request, _| {
            if matches!(request, Request::Commit(..)) {
                sample(&source, "新增.txt", b"do not delete");
            }
            false
        },
    );
    assert!(outcome.retry, "{}", outcome.message);
    assert_eq!(fs::read(source.join("新增.txt")).unwrap(), b"do not delete");
    assert_eq!(fs::read(parent.join("videos/one.mp4")).unwrap(), b"one");
    assert!(db.migration_journal().unwrap().unwrap().committed);
    fs::rename(source.join("新增.txt"), temp.path().join("保留.txt")).unwrap();
    let outcome = drive(&mut db, Worker::recover().unwrap(), |_, _| false);
    assert!(!outcome.retry, "{}", outcome.message);
    assert!(!source.exists());
    assert!(db.migration_journal().unwrap().is_none());
}

#[test]
fn changed_destination_is_not_deleted_on_failed_commit() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("videos");
    let parent = temp.path().join("target");
    sample(&source, "one.mp4", b"original");
    fs::create_dir(&parent).unwrap();
    let mut db = Database::open(&temp.path().join("db")).unwrap();
    db.add_root(&source).unwrap();
    let outcome = drive(
        &mut db,
        Worker::start(source.clone(), parent.clone(), vec![], true).unwrap(),
        |request, _| {
            if let Request::Commit(_, reply) = request {
                fs::write(parent.join("videos/one.mp4"), b"external edit").unwrap();
                reply.send(Err("injected failure".into())).unwrap();
                return true;
            }
            false
        },
    );
    assert!(outcome.retry);
    assert_eq!(fs::read(source.join("one.mp4")).unwrap(), b"original");
    assert_eq!(
        fs::read(parent.join("videos/one.mp4")).unwrap(),
        b"external edit"
    );
    fs::write(parent.join("videos/one.mp4"), b"original").unwrap();
    let outcome = drive(&mut db, Worker::recover().unwrap(), |_, _| false);
    assert!(!outcome.retry, "{}", outcome.message);
    assert!(!parent.join("videos").exists());
}

#[test]
fn occupied_files_are_preserved_and_empty_trees_move() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("videos");
    let parent = temp.path().join("target");
    let path = sample(&source, "busy.mp4", b"busy");
    fs::create_dir(&parent).unwrap();
    let mut db = Database::open(&temp.path().join("db")).unwrap();
    db.add_root(&source).unwrap();
    let handle = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let outcome = run(&mut db, &source, &parent, true);
    assert!(!outcome.retry, "{}", outcome.message);
    assert!(path.exists());
    drop(handle);
    fs::remove_file(path).unwrap();
    let outcome = run(&mut db, &source, &parent, true);
    assert!(!outcome.retry, "{}", outcome.message);
    assert!(parent.join("videos").is_dir());
    assert!(!source.exists());
}

#[test]
fn path_boundaries_are_case_insensitive_and_long_paths_move() {
    assert_eq!(
        migration::relative_under(Path::new(r"C:\VIDEOS\目录\A.mp4"), Path::new(r"c:\videos")),
        Some(PathBuf::from(r"目录\A.mp4"))
    );
    assert!(
        migration::relative_under(Path::new(r"C:\videos-old\a.mp4"), Path::new(r"c:\videos"))
            .is_none()
    );
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("videos");
    let parent = temp.path().join("target");
    let name = format!(
        "{}/{}/{}/video.mp4",
        "a".repeat(90),
        "b".repeat(90),
        "c".repeat(90)
    );
    let path = sample(&source, &name, b"long path");
    fs::create_dir(&parent).unwrap();
    let mut db = Database::open(&temp.path().join("db")).unwrap();
    db.add_root(&source).unwrap();
    let id = observe(&mut db, &path);
    db.assign(&[id], TagAssignment::Keep, true).unwrap();
    let outcome = run(&mut db, &source, &parent, true);
    assert!(!outcome.retry, "{}", outcome.message);
    let file = db
        .files()
        .unwrap()
        .into_iter()
        .find(|f| f.video_id == id)
        .unwrap();
    assert_eq!(file.path, parent.join("videos").join(&name));
    assert_eq!(file.state, FileState::Managed);
    assert_eq!(
        fs::read(parent.join("videos").join(name)).unwrap(),
        b"long path"
    );
}

#[test]
fn junction_is_rejected_without_touching_its_external_target() {
    use std::os::windows::process::CommandExt;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("videos");
    let parent = temp.path().join("target");
    let external = temp.path().join("external");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&parent).unwrap();
    sample(&external, "keep.mp4", b"outside");
    let link = source.join("junction");
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:FRAME_TEST_LINK -Target $env:FRAME_TEST_TARGET | Out-Null"])
        .env("FRAME_TEST_LINK", &link).env("FRAME_TEST_TARGET", &external).creation_flags(0x08000000).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut db = Database::open(&temp.path().join("db")).unwrap();
    db.add_root(&source).unwrap();
    let outcome = run(&mut db, &source, &parent, true);
    assert!(outcome.message.contains("junction"), "{}", outcome.message);
    assert_eq!(fs::read(external.join("keep.mp4")).unwrap(), b"outside");
    assert!(!parent.join("videos").exists());
    fs::remove_dir(link).unwrap();
}

#[test]
fn service_moves_paths_resumes_watching_and_joins_on_shutdown() {
    use video_management::service::{Command, Event, Service};
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("videos");
    let parent = temp.path().join("target");
    fs::create_dir(&parent).unwrap();
    let data = temp.path().join("data");
    fs::create_dir(&data).unwrap();
    let path = sample(&source, "one.mp4", b"managed");
    let mut db = Database::open(&data.join("library.sqlite3")).unwrap();
    db.add_root(&source).unwrap();
    let id = observe(&mut db, &path);
    db.assign(&[id], TagAssignment::Keep, true).unwrap();
    drop(db);
    let mut service = Service::start(data.clone()).unwrap();
    service.send(Command::MigrateRoot {
        source: source.clone(),
        parent: parent.clone(),
    });
    loop {
        match service
            .events
            .recv_timeout(Duration::from_secs(15))
            .unwrap()
        {
            Event::MigrationFinished(outcome) => {
                assert!(!outcome.retry, "{}", outcome.message);
                break;
            }
            Event::Error(error) => panic!("{error}"),
            _ => {}
        }
    }
    let target = parent.join("videos");
    assert!(!source.exists());
    let added = sample(&target, "new.mp4", b"new video");
    filetime::set_file_mtime(
        &added,
        filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - Duration::from_secs(10),
        ),
    )
    .unwrap();
    loop {
        match service
            .events
            .recv_timeout(Duration::from_secs(15))
            .unwrap()
        {
            Event::Snapshot(snapshot)
                if snapshot
                    .videos
                    .iter()
                    .any(|v| v.files.iter().any(|f| f.path == added)) =>
            {
                let managed = snapshot.videos.iter().find(|v| v.id == id).unwrap();
                assert!(!managed.pending());
                assert!(!managed.missing());
                assert_eq!(managed.files[0].path, target.join("one.mp4"));
                break;
            }
            Event::Error(error) => panic!("{error}"),
            _ => {}
        }
    }
    service.shutdown();
    assert!(
        service
            .events
            .try_iter()
            .any(|e| matches!(e, Event::Stopped))
    );
    assert!(
        Database::open(&data.join("library.sqlite3"))
            .unwrap()
            .migration_journal()
            .unwrap()
            .is_none()
    );
}

#[test]
fn native_copy_preserves_named_data_streams() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("videos");
    let parent = temp.path().join("target");
    sample(&source, "one.mp4", b"movie");
    fs::write(
        source.join("one.mp4:Zone.Identifier"),
        b"[ZoneTransfer]\r\nZoneId=3",
    )
    .unwrap();
    fs::create_dir(&parent).unwrap();
    let mut db = Database::open(&temp.path().join("db")).unwrap();
    db.add_root(&source).unwrap();
    let outcome = run(&mut db, &source, &parent, true);
    assert!(!outcome.retry, "{}", outcome.message);
    assert!(!source.exists(), "{}", outcome.message);
    assert_eq!(
        fs::read(parent.join("videos/one.mp4:Zone.Identifier")).unwrap(),
        b"[ZoneTransfer]\r\nZoneId=3"
    );
}

#[test]
fn startup_recovers_filesystem_moves_before_and_after_database_commit() {
    use video_management::service::{Event, Service};
    for copy in [false, true] {
        for committed in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("videos");
            let parent = temp.path().join("target");
            let data = temp.path().join("data");
            fs::create_dir(&data).unwrap();
            fs::create_dir(&parent).unwrap();
            let path = sample(&source, "one.mp4", b"recover me");
            let mut db = Database::open(&data.join("library.sqlite3")).unwrap();
            db.add_root(&source).unwrap();
            let id = observe(&mut db, &path);
            db.assign(&[id], TagAssignment::Keep, true).unwrap();
            let worker = Worker::start(source.clone(), parent.clone(), vec![], copy).unwrap();
            loop {
                let request = worker.events.recv_timeout(Duration::from_secs(15)).unwrap();
                if let Request::Commit(journal, reply) = request {
                    if committed {
                        db.commit_migration(&journal).unwrap();
                    }
                    // Disconnect without acknowledging: simulates loss of the process between
                    // filesystem promotion and receiving the durable transaction result.
                    drop(reply);
                    drop(worker);
                    break;
                }
                migration::answer(&mut db, request);
            }
            assert!(db.migration_journal().unwrap().is_some());
            drop(db);
            let mut service = Service::start(data.clone()).unwrap();
            loop {
                match service
                    .events
                    .recv_timeout(Duration::from_secs(15))
                    .unwrap()
                {
                    Event::MigrationFinished(outcome) => {
                        assert!(!outcome.retry, "{}", outcome.message);
                        break;
                    }
                    Event::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
            service.shutdown();
            let db = Database::open(&data.join("library.sqlite3")).unwrap();
            let retained = if committed {
                parent.join("videos")
            } else {
                source.clone()
            };
            assert_eq!(fs::read(retained.join("one.mp4")).unwrap(), b"recover me");
            assert_eq!(
                db.snapshot().unwrap().roots,
                std::slice::from_ref(&retained)
            );
            assert_eq!(db.files().unwrap()[0].path, retained.join("one.mp4"));
            assert!(db.migration_journal().unwrap().is_none());
            assert_eq!(source.exists(), !committed);
            assert_eq!(parent.join("videos").exists(), committed);
        }
    }
}

#[test]
fn shutdown_during_migration_joins_worker_and_leaves_consistent_paths() {
    use video_management::service::{Command, Event, Service};
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("videos");
    let parent = temp.path().join("target");
    let data = temp.path().join("data");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&parent).unwrap();
    sample(&source, "one.mp4", b"one");
    let db = Database::open(&data.join("library.sqlite3")).unwrap();
    db.add_root(&source).unwrap();
    drop(db);
    let mut service = Service::start(data.clone()).unwrap();
    service.send(Command::MigrateRoot {
        source: source.clone(),
        parent: parent.clone(),
    });
    loop {
        if matches!(
            service
                .events
                .recv_timeout(Duration::from_secs(15))
                .unwrap(),
            Event::MigrationProgress(_)
        ) {
            break;
        }
    }
    service.shutdown();
    assert!(
        service
            .events
            .try_iter()
            .any(|event| matches!(event, Event::Stopped))
    );
    let db = Database::open(&data.join("library.sqlite3")).unwrap();
    assert!(db.migration_journal().unwrap().is_none());
    let root = &db.snapshot().unwrap().roots[0];
    assert_eq!(fs::read(root.join("one.mp4")).unwrap(), b"one");
    assert!(source.exists() ^ parent.join("videos").exists());
}

#[test]
fn copying_failure_and_cancellation_clean_only_the_created_staging_tree() {
    for cancel in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("videos");
        let parent = temp.path().join("target");
        sample(&source, "one.mp4", &[19; 65536]);
        fs::create_dir(&parent).unwrap();
        let mut db = Database::open(&temp.path().join("db")).unwrap();
        db.add_root(&source).unwrap();
        let outcome = drive(
            &mut db,
            Worker::start(source.clone(), parent.clone(), vec![], true).unwrap(),
            |request, worker| {
                if let Request::Save(journal, reply) = request {
                    let saved = serde_json::to_value(journal).unwrap();
                    if saved["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|e| !e["destination_id"].is_null())
                    {
                        if cancel {
                            worker.cancel();
                        } else {
                            reply
                                .send(Err("injected copying checkpoint failure".into()))
                                .unwrap();
                            return true;
                        }
                    }
                }
                false
            },
        );
        assert!(!outcome.retry, "{}", outcome.message);
        assert_eq!(fs::read(source.join("one.mp4")).unwrap(), [19; 65536]);
        assert!(fs::read_dir(&parent).unwrap().next().is_none());
        assert!(db.migration_journal().unwrap().is_none());
    }
}

#[test]
fn whole_tree_verification_rejects_a_changed_staged_file_before_promotion() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("videos");
    let parent = temp.path().join("target");
    sample(&source, "a.mp4", b"original");
    fs::create_dir(source.join("z-directory")).unwrap();
    fs::create_dir(&parent).unwrap();
    let mut db = Database::open(&temp.path().join("db")).unwrap();
    db.add_root(&source).unwrap();
    let mut changed = None;
    let outcome = drive(
        &mut db,
        Worker::start(source.clone(), parent.clone(), vec![], true).unwrap(),
        |request, _| {
            if let Request::Save(journal, _) = request {
                let saved = serde_json::to_value(journal).unwrap();
                if changed.is_none()
                    && saved["entries"][0]["hash"].is_string()
                    && !saved["entries"][1]["destination_id"].is_null()
                {
                    let path = PathBuf::from(saved["staging"].as_str().unwrap()).join("a.mp4");
                    fs::write(&path, b"tampered").unwrap();
                    changed = Some(path);
                }
            }
            false
        },
    );
    assert!(outcome.retry, "{}", outcome.message);
    assert!(!parent.join("videos").exists());
    assert_eq!(fs::read(source.join("a.mp4")).unwrap(), b"original");
    let path = changed.unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"tampered");
    fs::write(path, b"original").unwrap();
    let outcome = drive(&mut db, Worker::recover().unwrap(), |_, _| false);
    assert!(!outcome.retry, "{}", outcome.message);
    assert!(fs::read_dir(&parent).unwrap().next().is_none());
}
