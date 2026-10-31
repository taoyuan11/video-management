use semver::Version;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};
use video_management::{
    database::Database,
    model::{Settings, UpdateSettings},
    service::{Command, Event, Service},
    update::{self, Job, Package, PackageFile, Release},
};

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn json(path: &Path, value: &impl serde::Serialize) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
fn package(version: &str, files: &[(&str, &[u8])]) -> Package {
    Package {
        product: update::PRODUCT.into(),
        platform: update::PLATFORM.into(),
        version: Version::parse(version).unwrap(),
        files: files
            .iter()
            .map(|(path, bytes)| PackageFile {
                path: (*path).into(),
                size: bytes.len() as u64,
                sha256: digest(bytes),
            })
            .collect(),
    }
}
fn write_tree(root: &Path, version: &str, files: &[(&str, &[u8])]) {
    fs::create_dir_all(root).unwrap();
    for (name, bytes) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    json(
        &root.join(update::PACKAGE_MANIFEST),
        &package(version, files),
    );
}
const OLD: &[(&str, &[u8])] = &[
    (update::MAIN_EXE, b"old program"),
    (update::UPDATER_EXE, b"old updater"),
    ("lib/obsolete.dll", b"old plugin"),
];
const NEW: &[(&str, &[u8])] = &[
    (update::MAIN_EXE, b"new program"),
    (update::UPDATER_EXE, b"new updater"),
    ("lib/new.dll", b"new plugin"),
];
fn archive(
    root: &Path,
    version: &str,
    files: &[(&str, &[u8])],
    extra: Option<(&str, &[u8])>,
) -> PathBuf {
    let path = root.join(format!("FrameLibrary-{version}-windows-x64.zip"));
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file(update::PACKAGE_MANIFEST, options).unwrap();
    zip.write_all(&serde_json::to_vec(&package(version, files)).unwrap())
        .unwrap();
    zip.add_directory("empty-runtime-directory/", options)
        .unwrap();
    for (name, bytes) in files.iter().copied().chain(extra) {
        zip.start_file(name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    path
}
fn publish(root: &Path, version: &str) -> Release {
    let path = archive(root, version, NEW, None);
    let release = Release {
        product: update::PRODUCT.into(),
        platform: update::PLATFORM.into(),
        version: Version::parse(version).unwrap(),
        archive: path.file_name().unwrap().to_string_lossy().into(),
        size: fs::metadata(&path).unwrap().len(),
        sha256: update::hash_file(&path, &AtomicBool::new(false)).unwrap(),
    };
    json(&root.join(format!("{version}.update.json")), &release);
    release
}
fn fixture() -> (tempfile::TempDir, PathBuf, Job) {
    let temp = tempfile::tempdir().unwrap();
    let install = temp.path().join("程序 中文");
    write_tree(&install, "0.1.0", OLD);
    let data = install.join("data");
    fs::create_dir(&data).unwrap();
    fs::write(data.join("library.sqlite3"), b"precious database").unwrap();
    fs::write(install.join("notes.txt"), b"user notes").unwrap();
    let work = install.join(".frame-update-test");
    write_tree(&work.join("payload"), "0.2.0", NEW);
    let job = Job {
        install,
        data,
        helper: temp.path().join("unused"),
        version: Version::parse("0.2.0").unwrap(),
    };
    (temp, work, job)
}

#[test]
fn highest_semantic_version_selected_without_recursing() {
    let temp = tempfile::tempdir().unwrap();
    for v in ["0.1.0", "0.2.0", "0.10.0", "1.0.0-beta.1"] {
        publish(temp.path(), v);
    }
    fs::create_dir(temp.path().join("nested")).unwrap();
    publish(&temp.path().join("nested"), "9.0.0");
    fs::write(temp.path().join("bad.update.json"), b"not json").unwrap();
    let result = update::check(temp.path(), "0.1.0", &AtomicBool::new(false)).unwrap();
    assert_eq!(
        result.candidate.unwrap().release.version.to_string(),
        "0.10.0"
    );
    assert_eq!(result.ignored, 1);
    assert!(
        update::check(temp.path(), "0.10.0+build.2", &AtomicBool::new(false))
            .unwrap()
            .candidate
            .is_none()
    );
}
#[test]
fn incomplete_archive_wrong_product_and_offline_folder() {
    let temp = tempfile::tempdir().unwrap();
    let mut release = publish(temp.path(), "1.0.0");
    fs::write(temp.path().join(&release.archive), b"partial").unwrap();
    let result = update::check(temp.path(), "0.1.0", &AtomicBool::new(false)).unwrap();
    assert!(result.candidate.is_none());
    assert_eq!(result.ignored, 1);
    release.product = "OtherProduct".into();
    json(&temp.path().join("1.0.0.update.json"), &release);
    assert!(
        update::check(temp.path(), "0.1.0", &AtomicBool::new(false))
            .unwrap()
            .candidate
            .is_none()
    );
    assert!(
        update::check(
            &temp.path().join("offline"),
            "0.1.0",
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(update::check(temp.path(), "0.1.0", &AtomicBool::new(true)).is_err());
}
#[test]
fn windows_unsafe_paths_are_rejected() {
    for name in [
        "../data/db",
        "C:/bad",
        "//server/share/a",
        "\\bad",
        "data/a",
        "DATA/a",
        "dir/../../a",
        "a:stream",
        "a./b",
        "a /b",
        "con.txt",
        "dir/LPT1",
        "COM¹.log",
        ".frame-install.lock",
        ".frame-update-test/a",
    ] {
        assert!(update::relative_path(name).is_err(), "accepted {name}");
    }
    assert!(update::relative_path("lib/中文 文件.dll").is_ok());
}
#[test]
fn verified_zip_extracts_and_rejects_unlisted_or_traversing_files() {
    let temp = tempfile::tempdir().unwrap();
    let mut files = NEW.to_vec();
    files.push(("lib/中文 目录/组件.dll", b"unicode plugin"));
    let good = archive(temp.path(), "0.2.0", &files, None);
    let out = temp.path().join("out");
    fs::create_dir(&out).unwrap();
    update::extract(
        &good,
        &out,
        &Version::parse("0.2.0").unwrap(),
        &AtomicBool::new(false),
        &|_| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(out.join(update::MAIN_EXE)).unwrap(),
        b"new program"
    );
    assert_eq!(
        fs::read(out.join("lib/中文 目录/组件.dll")).unwrap(),
        b"unicode plugin"
    );
    for (i, name) in [
        "../escape",
        "unknown.txt",
        "DATA/stolen",
        "VIDEO-MANAGEMENT.EXE",
    ]
    .iter()
    .enumerate()
    {
        let zip = archive(temp.path(), "0.3.0", NEW, Some((name, b"evil")));
        let out = temp.path().join(format!("bad{i}"));
        fs::create_dir(&out).unwrap();
        assert!(
            update::extract(
                &zip,
                &out,
                &Version::parse("0.3.0").unwrap(),
                &AtomicBool::new(false),
                &|_| {}
            )
            .is_err()
        );
    }
    assert!(!temp.path().join("escape").exists());
}
#[test]
fn package_version_file_hash_and_symlink_are_checked() {
    let temp = tempfile::tempdir().unwrap();
    let zip = archive(temp.path(), "0.2.0", NEW, None);
    let out = temp.path().join("out");
    fs::create_dir(&out).unwrap();
    assert!(
        update::extract(
            &zip,
            &out,
            &Version::parse("0.3.0").unwrap(),
            &AtomicBool::new(false),
            &|_| {}
        )
        .is_err()
    );
    let path = temp.path().join("bad.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    writer
        .start_file(update::PACKAGE_MANIFEST, options)
        .unwrap();
    writer
        .write_all(&serde_json::to_vec(&package("0.2.0", NEW)).unwrap())
        .unwrap();
    writer
        .add_symlink(update::MAIN_EXE, "outside", options)
        .unwrap();
    writer.finish().unwrap();
    assert!(
        update::extract(
            &path,
            &out,
            &Version::parse("0.2.0").unwrap(),
            &AtomicBool::new(false),
            &|_| {}
        )
        .is_err()
    );
    let (_temp, work, job) = fixture();
    fs::write(work.join("payload").join(update::MAIN_EXE), b"tampered").unwrap();
    assert!(update::install(&work, &job, || Ok(())).is_err());
    assert_eq!(
        fs::read(job.install.join(update::MAIN_EXE)).unwrap(),
        b"old program"
    );
}
#[test]
fn full_update_preserves_data_and_removes_only_managed_obsolete_files() {
    let (_temp, work, job) = fixture();
    update::install(&work, &job, || Ok(())).unwrap();
    assert_eq!(
        fs::read(job.install.join(update::MAIN_EXE)).unwrap(),
        b"new program"
    );
    assert!(!job.install.join("lib/obsolete.dll").exists());
    assert_eq!(
        fs::read(job.data.join("library.sqlite3")).unwrap(),
        b"precious database"
    );
    assert_eq!(
        fs::read(job.install.join("notes.txt")).unwrap(),
        b"user notes"
    );
}
#[test]
fn failed_restart_restores_every_replaced_and_removed_file() {
    let (_temp, work, job) = fixture();
    let old_manifest = fs::read(job.install.join(update::PACKAGE_MANIFEST)).unwrap();
    let result = update::install(&work, &job, || anyhow::bail!("injected launch failure"));
    assert!(result.unwrap_err().to_string().contains("旧版本已保留"));
    for (name, bytes) in OLD {
        assert_eq!(&fs::read(job.install.join(name)).unwrap(), bytes);
    }
    assert!(!job.install.join("lib/new.dll").exists());
    assert_eq!(
        fs::read(job.install.join(update::PACKAGE_MANIFEST)).unwrap(),
        old_manifest
    );
}
#[test]
fn locked_and_readonly_files_leave_installation_untouched() {
    let (_temp, work, job) = fixture();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(job.install.join(update::MAIN_EXE))
        .unwrap();
    assert!(update::install(&work, &job, || Ok(())).is_err());
    drop(lock);
    let path = job.install.join(update::MAIN_EXE);
    let original = fs::metadata(&path).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&path, readonly).unwrap();
    assert!(update::install(&work, &job, || Ok(())).is_err());
    fs::set_permissions(&path, original).unwrap();
    assert_eq!(fs::read(path).unwrap(), b"old program");
}
#[test]
fn custom_data_and_unmanaged_collisions_are_protected() {
    let (_temp, work, mut job) = fixture();
    job.data = job.install.join("lib");
    assert!(update::install(&work, &job, || Ok(())).is_err());
    job.data = job.install.join("data");
    fs::write(job.install.join("lib/new.dll"), b"user file").unwrap();
    assert!(update::install(&work, &job, || Ok(())).is_err());
    assert_eq!(
        fs::read(job.install.join("lib/new.dll")).unwrap(),
        b"user file"
    );
}
#[test]
fn installation_lock_prevents_updating_another_running_instance() {
    let temp = tempfile::tempdir().unwrap();
    let a = update::installation_lock(temp.path(), false).unwrap();
    let b = update::installation_lock(temp.path(), false).unwrap();
    assert!(update::installation_lock(temp.path(), true).is_err());
    drop(a);
    drop(b);
    let exclusive = update::installation_lock(temp.path(), true).unwrap();
    assert!(update::installation_lock(temp.path(), false).is_err());
    drop(exclusive);
}
#[test]
fn settings_roundtrip_preserves_legacy_player_settings() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.sqlite3");
    let db = Database::open(&path).unwrap();
    db.save_settings(&Settings {
        volume: 0.25,
        muted: true,
    })
    .unwrap();
    assert_eq!(db.update_settings().unwrap(), UpdateSettings::default());
    let settings = UpdateSettings {
        folder: Some(temp.path().join("更新文件夹")),
    };
    db.save_update_settings(&settings).unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.update_settings().unwrap(), settings);
    assert_eq!(db.settings().unwrap().volume, 0.25);
    assert!(db.settings().unwrap().muted);
    db.save_update_settings(&UpdateSettings::default()).unwrap();
    assert_eq!(db.update_settings().unwrap(), UpdateSettings::default());
}
#[test]
fn update_shutdown_flushes_queued_writes_and_releases_database_lock() {
    let temp = tempfile::tempdir().unwrap();
    let mut service = Service::start(temp.path().to_path_buf()).unwrap();
    loop {
        if matches!(
            service.events.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::UpdateSettings(Ok(_))
        ) {
            break;
        }
    }
    let settings = UpdateSettings {
        folder: Some(temp.path().join("source")),
    };
    service.send(Command::UpdateSettings(settings.clone()));
    service.send(Command::Settings(Settings {
        volume: 0.33,
        muted: true,
    }));
    service.send(Command::CreateTag("更新前标签".into()));
    service.shutdown_for_update().unwrap();
    assert!(
        service
            .events
            .try_iter()
            .any(|e| matches!(e, Event::UpdateSettingsSaved(Ok(_))))
    );
    let db = Database::open(&temp.path().join("library.sqlite3")).unwrap();
    assert_eq!(db.update_settings().unwrap(), settings);
    assert_eq!(db.settings().unwrap().volume, 0.33);
    assert_eq!(db.snapshot().unwrap().tags[0].name, "更新前标签");
    drop(db);
    let mut next = Service::start(temp.path().to_path_buf()).unwrap();
    next.shutdown_for_update().unwrap();
}
#[test]
fn preparation_rejects_corrupt_zip_without_changing_program() {
    let (_temp, _work, job) = fixture();
    let source = tempfile::tempdir().unwrap();
    let mut release = publish(source.path(), "0.2.0");
    release.sha256 = "00".repeat(32);
    let candidate = update::Candidate {
        release,
        folder: source.path().to_path_buf(),
    };
    assert!(
        update::prepare(
            &candidate,
            &job.install,
            &job.data,
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err()
    );
    assert_eq!(
        fs::read(job.install.join(update::MAIN_EXE)).unwrap(),
        b"old program"
    );
}

#[test]
fn blocked_parent_preserves_installation_and_user_file() {
    let (_temp, work, job) = fixture();
    let files: Vec<_> = NEW
        .iter()
        .copied()
        .chain([("zz/block/file.dll", &b"plugin"[..])])
        .collect();
    write_tree(&work.join("payload"), "0.2.0", &files);
    fs::create_dir(job.install.join("zz")).unwrap();
    fs::write(job.install.join("zz/block"), b"user file blocks directory").unwrap();
    assert!(update::install(&work, &job, || Ok(())).is_err());
    for (name, bytes) in OLD {
        assert_eq!(&fs::read(job.install.join(name)).unwrap(), bytes);
    }
    assert!(!job.install.join("lib/new.dll").exists());
    assert_eq!(
        fs::read(job.install.join("zz/block")).unwrap(),
        b"user file blocks directory"
    );
}

#[test]
fn real_release_archive_is_complete_when_supplied() {
    let Some(path) = std::env::var_os("FRAME_LIBRARY_TEST_ARCHIVE") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    update::extract(
        Path::new(&path),
        temp.path(),
        &Version::parse(update::VERSION).unwrap(),
        &AtomicBool::new(false),
        &|_| {},
    )
    .unwrap();
    assert!(temp.path().join(update::MAIN_EXE).is_file());
    assert!(temp.path().join(update::UPDATER_EXE).is_file());
    assert!(!temp.path().join("data").exists());
}
