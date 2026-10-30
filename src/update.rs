//! Local, manifest-based portable updates. No GUI or playback dependencies.
use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Storage::FileSystem::GetDriveTypeW,
    System::Threading::{CREATE_NO_WINDOW, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
};

pub const PRODUCT: &str = "FrameLibrary";
pub const PLATFORM: &str = "windows-x64";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAIN_EXE: &str = "video-management.exe";
pub const UPDATER_EXE: &str = "frame-library-updater.exe";
pub const PACKAGE_MANIFEST: &str = "release.json";
const MAX_JSON: u64 = 16 * 1024 * 1024;
const MAX_UNPACKED: u64 = 16 * 1024 * 1024 * 1024;
const JOB_PREFIX: &str = ".frame-update-";
const HELPER_PREFIX: &str = "FrameLibrary-Updater-";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub product: String,
    pub version: Version,
    pub platform: String,
    pub archive: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub product: String,
    pub version: Version,
    pub platform: String,
    pub files: Vec<PackageFile>,
}
#[derive(Debug, Clone)]
pub struct Candidate {
    pub release: Release,
    pub folder: PathBuf,
}
#[derive(Debug)]
pub struct CheckResult {
    pub candidate: Option<Candidate>,
    pub ignored: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Job {
    pub install: PathBuf,
    pub data: PathBuf,
    pub helper: PathBuf,
    pub version: Version,
}
#[derive(Debug)]
pub struct PreparedUpdate {
    pub directory: PathBuf,
    pub job: Job,
    keep: bool,
}
impl Drop for PreparedUpdate {
    fn drop(&mut self) {
        if !self.keep {
            let _ = cleanup_job(&self.directory, &self.job.install);
        }
    }
}

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
pub fn show_error(message: &str) {
    let title = wide(std::ffi::OsStr::new("帧库 · 软件更新"));
    let message = wide(std::ffi::OsStr::new(message));
    // Both strings are NUL terminated and valid for the duration of the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "已取消更新操作");
    Ok(())
}

/// Resolve junctions before accepting a local fixed/removable drive.
pub fn local_folder(path: &Path) -> Result<PathBuf> {
    local_drive(path)?;
    let path = dunce::canonicalize(path).context("更新文件夹无法访问，请检查磁盘是否已连接")?;
    ensure!(path.is_dir(), "请选择文件夹");
    local_drive(&path)?;
    Ok(path)
}
fn local_drive(path: &Path) -> Result<()> {
    let display = path.as_os_str().encode_wide().collect::<Vec<_>>();
    ensure!(
        display.len() >= 3 && display[1] == b':' as u16 && display[2] == b'\\' as u16,
        "仅支持本机磁盘和移动硬盘，不支持网络共享"
    );
    let drive = [display[0], display[1], display[2], 0];
    let kind = unsafe { GetDriveTypeW(drive.as_ptr()) };
    ensure!(
        kind == 2 || kind == 3,
        "仅支持本机磁盘和移动硬盘，不支持网络映射盘"
    );
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_JSON + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_JSON, "版本清单过大");
    // Windows PowerShell can emit UTF-8 BOMs.
    Ok(serde_json::from_slice(
        bytes.strip_prefix(&[239, 187, 191]).unwrap_or(&bytes),
    )?)
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = File::create(path)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    Ok(())
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Windows-relative paths only; reject alternate streams, device names and aliases.
pub fn relative_path(value: &str) -> Result<PathBuf> {
    let normalized = value.replace('\\', "/");
    ensure!(!normalized.is_empty(), "空文件路径");
    let mut result = PathBuf::new();
    for component in normalized.split('/') {
        ensure!(
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.ends_with([' ', '.'])
                && !component.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c)),
            "不安全的更新文件路径：{value}"
        );
        let upper = component.split('.').next().unwrap_or("").to_uppercase();
        let reserved = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&upper.as_str())
            || ["COM", "LPT"].iter().any(|prefix| {
                upper
                    .strip_prefix(prefix)
                    .is_some_and(|n| n.chars().count() == 1 && "123456789¹²³".contains(n))
            });
        ensure!(!reserved, "不安全的设备路径：{value}");
        result.push(component);
    }
    let first = normalized.split('/').next().unwrap().to_lowercase();
    ensure!(
        first != "data" && first != ".frame-install.lock" && !first.starts_with(JOB_PREFIX),
        "更新包不得覆盖用户数据或更新工作目录：{value}"
    );
    Ok(result)
}
fn path_id(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}
fn validate_package(package: &Package) -> Result<BTreeMap<String, &PackageFile>> {
    ensure!(
        package.product == PRODUCT
            && package.platform == PLATFORM
            && package.version.pre.is_empty(),
        "更新包产品、平台或版本不匹配"
    );
    let mut files = BTreeMap::new();
    let mut size = 0_u64;
    for entry in &package.files {
        let path = relative_path(&entry.path)?;
        let key = path_id(&path);
        ensure!(
            key != PACKAGE_MANIFEST && valid_hash(&entry.sha256),
            "无效的文件清单"
        );
        ensure!(
            files.insert(key, entry).is_none(),
            "清单包含重复文件：{}",
            entry.path
        );
        size = size.checked_add(entry.size).context("文件大小溢出")?;
    }
    ensure!(size <= MAX_UNPACKED, "更新包解压后过大");
    for key in files.keys() {
        let mut parent = Path::new(key).parent();
        while let Some(path) = parent {
            ensure!(
                !files.contains_key(&path_id(path)),
                "清单中的文件与目录冲突"
            );
            parent = path.parent();
        }
    }
    ensure!(
        files.contains_key(MAIN_EXE) && files.contains_key(UPDATER_EXE),
        "更新包缺少主程序或更新器"
    );
    Ok(files)
}

pub fn check(folder: &Path, current: &str, cancel: &AtomicBool) -> Result<CheckResult> {
    let folder = local_folder(folder)?;
    let current = Version::parse(current)?;
    let mut result = CheckResult {
        candidate: None,
        ignored: 0,
    };
    let mut entries = fs::read_dir(&folder)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        cancelled(cancel)?;
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with(".update.json")
        {
            continue;
        }
        let candidate = (|| -> Result<Option<Candidate>> {
            no_reparse(&entry.path())?;
            let release: Release = read_json(&entry.path())?;
            ensure!(
                release.product == PRODUCT && release.platform == PLATFORM,
                "产品或平台不匹配"
            );
            if !release.version.pre.is_empty() || release.version.cmp_precedence(&current).is_le() {
                return Ok(None);
            }
            let path = relative_path(&release.archive)?;
            ensure!(
                path.components().count() == 1
                    && release.archive.ends_with(".zip")
                    && valid_hash(&release.sha256),
                "无效的发布清单"
            );
            let archive = folder.join(path);
            no_reparse(&archive)?;
            let meta = fs::metadata(&archive)?;
            ensure!(
                meta.is_file() && meta.len() == release.size && release.size > 0,
                "发布包尚未复制完成"
            );
            Ok(Some(Candidate {
                release,
                folder: folder.clone(),
            }))
        })();
        match candidate {
            Ok(Some(candidate))
                if result.candidate.as_ref().is_none_or(|old| {
                    candidate
                        .release
                        .version
                        .cmp_precedence(&old.release.version)
                        .is_gt()
                }) =>
            {
                result.candidate = Some(candidate)
            }
            Err(_) => result.ignored += 1,
            _ => {}
        }
    }
    Ok(result)
}

pub fn hash_file(path: &Path, cancel: &AtomicBool) -> Result<String> {
    let mut input = OpenOptions::new().read(true).share_mode(1).open(path)?;
    hash_reader(&mut input, cancel)
}
fn hash_reader(input: &mut impl Read, cancel: &AtomicBool) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        cancelled(cancel)?;
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
fn copy_checked(source: &Path, destination: &Path, cancel: &AtomicBool) -> Result<()> {
    let mut input = OpenOptions::new().read(true).share_mode(1).open(source)?;
    let mut output = File::create(destination)?;
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        cancelled(cancel)?;
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        output.write_all(&buffer[..n])?;
    }
    output.sync_all()?;
    Ok(())
}
fn no_reparse(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.file_attributes() & 0x400 == 0,
        "更新路径包含链接或重解析点：{}",
        path.display()
    );
    Ok(())
}
fn safe_target(root: &Path, relative: &Path, data: &Path) -> Result<PathBuf> {
    let target = root.join(relative);
    // Compare Windows paths case-insensitively, including custom data directories.
    let target_key = path_id(&target);
    let data_key = path_id(data);
    ensure!(
        target_key != data_key && !target_key.starts_with(&(data_key + "/")),
        "更新包试图覆盖资料库"
    );
    let mut path = root.to_path_buf();
    no_reparse(&path)?;
    for part in relative.components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                no_reparse(&path)?;
                let resolved = path_id(&dunce::canonicalize(&path)?);
                let protected = path_id(data);
                ensure!(
                    resolved != protected && !resolved.starts_with(&(protected + "/")),
                    "更新路径指向资料库"
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(target)
}

pub fn prepare(
    candidate: &Candidate,
    install: &Path,
    data: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(String),
) -> Result<PreparedUpdate> {
    let install = local_folder(install)?;
    let data = dunce::canonicalize(data).context("资料库目录无法访问")?;
    let work = tempfile::Builder::new()
        .prefix(JOB_PREFIX)
        .tempdir_in(&install)?;
    let helper = tempfile::Builder::new().prefix(HELPER_PREFIX).tempdir()?;
    let archive_path = work.path().join("package.zip");
    progress("正在复制更新包…".into());
    let source = local_folder(&candidate.folder)?.join(relative_path(&candidate.release.archive)?);
    no_reparse(&source)?;
    copy_checked(&source, &archive_path, cancel)?;
    ensure!(
        fs::metadata(&archive_path)?.len() == candidate.release.size,
        "更新包大小已变化，请重新检查"
    );
    progress("正在校验更新包…".into());
    ensure!(
        hash_file(&archive_path, cancel)?.eq_ignore_ascii_case(&candidate.release.sha256),
        "更新包校验失败，请重新复制完整发布包"
    );
    let payload = work.path().join("payload");
    fs::create_dir(&payload)?;
    extract(
        &archive_path,
        &payload,
        &candidate.release.version,
        cancel,
        &progress,
    )?;
    let package: Package = read_json(&payload.join(PACKAGE_MANIFEST))?;
    for file in &package.files {
        safe_target(&install, &relative_path(&file.path)?, &data)?;
    }
    // Run the already installed updater, never execute unverified code from the source folder.
    copy_checked(
        &install.join(UPDATER_EXE),
        &helper.path().join(UPDATER_EXE),
        cancel,
    )
    .context("缺少更新器，请先手动部署包含更新功能的完整便携包")?;
    // The updater is built without GUI/GStreamer; copy its app-local MSVC runtime if present.
    for entry in fs::read_dir(&install)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if (name.starts_with("vcruntime") || name.starts_with("msvcp")) && name.ends_with(".dll") {
            copy_checked(
                &entry.path(),
                &helper.path().join(entry.file_name()),
                cancel,
            )?;
        }
    }
    let job = Job {
        install,
        data,
        helper: helper.path().to_path_buf(),
        version: package.version,
    };
    write_json(&work.path().join("job.json"), &job)?;
    let directory = work.keep();
    let _ = helper.keep();
    Ok(PreparedUpdate {
        directory,
        job,
        keep: false,
    })
}

pub fn extract(
    archive: &Path,
    destination: &Path,
    version: &Version,
    cancel: &AtomicBool,
    progress: &impl Fn(String),
) -> Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
    let package: Package = {
        let entry = zip.by_name(PACKAGE_MANIFEST).context("缺少包内版本清单")?;
        ensure!(entry.size() <= MAX_JSON, "包内清单过大");
        let mut bytes = Vec::new();
        entry.take(MAX_JSON + 1).read_to_end(&mut bytes)?;
        serde_json::from_slice(bytes.strip_prefix(&[239, 187, 191]).unwrap_or(&bytes))?
    };
    ensure!(&package.version == version, "包内版本与发布清单不一致");
    let expected = validate_package(&package)?;
    let mut seen = BTreeSet::new();
    let mut seen_files = BTreeSet::new();
    for index in 0..zip.len() {
        cancelled(cancel)?;
        let mut entry = zip.by_index(index)?;
        let name = entry.name().trim_end_matches(['/', '\\']);
        let relative = relative_path(name)?;
        let key = path_id(&relative);
        ensure!(seen.insert(key.clone()), "ZIP 包含重复目标：{name}");
        if let Some(mode) = entry.unix_mode() {
            ensure!(
                matches!(mode & 0o170000, 0 | 0o100000 | 0o040000),
                "ZIP 不支持链接或特殊文件"
            );
        }
        if entry.is_dir() {
            ensure!(
                !expected.contains_key(&key) && key != PACKAGE_MANIFEST,
                "无效的包内目录"
            );
            // .NET ZIP creation includes empty runtime directories. They carry no
            // files and need not be created or added to the managed file list.
            continue;
        }
        if key == PACKAGE_MANIFEST {
            continue;
        }
        let file = expected.get(&key).context("ZIP 包含未列入清单的文件")?;
        ensure!(entry.size() == file.size, "包内文件大小不符：{}", file.path);
        let path = destination.join(relative_path(&file.path)?);
        fs::create_dir_all(path.parent().unwrap())?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 1024 * 1024];
        let mut written = 0_u64;
        loop {
            cancelled(cancel)?;
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            ensure!(written <= file.size, "文件解压大小超出清单");
            hash.update(&buffer[..n]);
            output.write_all(&buffer[..n])?;
        }
        let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        ensure!(
            written == file.size && digest.eq_ignore_ascii_case(&file.sha256),
            "包内文件校验失败：{}",
            file.path
        );
        output.sync_all()?;
        seen_files.insert(key);
        if index % 50 == 0 {
            progress(format!(
                "正在解压并校验… {}/{}",
                seen_files.len(),
                expected.len()
            ));
        }
    }
    ensure!(seen_files.len() == expected.len(), "ZIP 缺少清单中的文件");
    write_json(&destination.join(PACKAGE_MANIFEST), &package)?;
    Ok(())
}

/// Shared by application instances; the updater requires exclusive access.
pub fn installation_lock(root: &Path, exclusive: bool) -> Result<File> {
    let path = root.join(".frame-install.lock");
    if path.exists() {
        no_reparse(&path)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    if exclusive {
        file.try_lock()?;
    } else {
        file.try_lock_shared()?;
    }
    Ok(file)
}

pub struct Handoff {
    pub prepared: PreparedUpdate,
    child: Child,
    committed: bool,
}
impl Handoff {
    pub fn commit(&mut self) -> Result<()> {
        ensure!(
            self.child.try_wait()?.is_none(),
            "更新器已退出，未执行文件替换"
        );
        self.committed = true;
        self.prepared.keep = true;
        Ok(())
    }
}
impl Drop for Handoff {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::write(self.prepared.directory.join("cancel"), b"cancel");
            let deadline = Instant::now() + Duration::from_secs(3);
            while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(50));
            }
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }
    }
}
pub fn launch_helper(prepared: PreparedUpdate, cancel: &AtomicBool) -> Result<Handoff> {
    let child = Command::new(prepared.job.helper.join(UPDATER_EXE))
        .arg("--apply")
        .arg(&prepared.directory)
        .arg(std::process::id().to_string())
        .current_dir(&prepared.job.helper)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let mut handoff = Handoff {
        prepared,
        child,
        committed: false,
    };
    let start = Instant::now();
    loop {
        cancelled(cancel)?;
        if handoff.prepared.directory.join("ready").exists() {
            return Ok(handoff);
        }
        ensure!(handoff.child.try_wait()?.is_none(), "更新器未能启动");
        ensure!(start.elapsed() < Duration::from_secs(15), "更新器启动超时");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Apply verified staged files. Restore all touched files if replacement or launch fails.
pub fn install(job_dir: &Path, job: &Job, launch: impl FnOnce() -> Result<()>) -> Result<()> {
    let payload = job_dir.join("payload");
    let package: Package = read_json(&payload.join(PACKAGE_MANIFEST))?;
    ensure!(package.version == job.version, "暂存版本不匹配");
    let entries = validate_package(&package)?;
    let cancel = AtomicBool::new(false);
    for entry in entries.values() {
        let path = safe_target(&payload, &relative_path(&entry.path)?, &job.data)?;
        ensure!(
            fs::metadata(&path)?.len() == entry.size
                && hash_file(&path, &cancel)?.eq_ignore_ascii_case(&entry.sha256),
            "暂存文件校验失败：{}",
            entry.path
        );
    }
    let mut targets: BTreeMap<String, PathBuf> = entries
        .iter()
        .map(|(k, v)| Ok((k.clone(), relative_path(&v.path)?)))
        .collect::<Result<_>>()?;
    let old_path = job.install.join(PACKAGE_MANIFEST);
    let mut managed = BTreeSet::new();
    if old_path.exists() {
        let old: Package = read_json(&old_path).context("旧版本清单损坏，无法安全更新")?;
        for (key, entry) in validate_package(&old)? {
            managed.insert(key.clone());
            targets.entry(key).or_insert(relative_path(&entry.path)?);
        }
    }
    targets.insert(PACKAGE_MANIFEST.into(), PathBuf::from(PACKAGE_MANIFEST));
    // Complete preflight before touching any installed file.
    for relative in targets.values() {
        let target = safe_target(&job.install, relative, &job.data)?;
        if target.exists() {
            ensure!(
                relative == Path::new(PACKAGE_MANIFEST) || managed.contains(&path_id(relative)),
                "更新目标与未受管理的文件冲突，文件已保留：{}",
                target.display()
            );
            ensure!(target.is_file(), "更新目标不是文件：{}", target.display());
            OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&target)
                .with_context(|| {
                    format!("文件被占用或不可写，请关闭其他程序：{}", target.display())
                })?;
        }
    }
    let backup = job_dir.join("backup");
    fs::create_dir(&backup)?;
    let mut journal = File::create(job_dir.join("rollback.jsonl"))?;
    let mut touched = Vec::<(PathBuf, bool)>::new();
    let result = (|| -> Result<()> {
        for (key, relative) in &targets {
            let target = safe_target(&job.install, relative, &job.data)?;
            let saved = backup.join(relative);
            let exists = target.exists();
            if exists {
                fs::create_dir_all(saved.parent().unwrap())?;
                fs::rename(&target, &saved)
                    .with_context(|| format!("无法备份 {}", target.display()))?;
            }
            touched.push((relative.clone(), exists));
            serde_json::to_writer(&mut journal, touched.last().unwrap())?;
            journal.write_all(b"\n")?;
            journal.sync_data()?;
            if key == PACKAGE_MANIFEST || entries.contains_key(key) {
                fs::create_dir_all(target.parent().unwrap())?;
                fs::rename(payload.join(relative), &target)
                    .with_context(|| format!("无法替换 {}", target.display()))?;
            }
        }
        launch()?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut failures = Vec::new();
        for (relative, existed) in touched.iter().rev() {
            let target = job.install.join(relative);
            let restore = (|| -> Result<()> {
                if target.exists() {
                    fs::remove_file(&target)?;
                }
                if *existed {
                    fs::rename(backup.join(relative), target)?;
                }
                Ok(())
            })();
            if let Err(e) = restore {
                failures.push(format!("{}：{e}", relative.display()));
            }
        }
        if !failures.is_empty() {
            fs::write(job_dir.join("recovery-required"), failures.join("\n"))?;
            bail!(
                "更新失败：{error:#}\n恢复旧文件失败，请保留备份：{}\n{}",
                backup.display(),
                failures.join("\n")
            );
        }
        bail!("更新失败，旧版本已保留：{error:#}");
    }
    Ok(())
}

pub fn run_helper(job_dir: &Path, parent: u32) -> Result<()> {
    let job: Job = read_json(&job_dir.join("job.json"))?;
    validate_job_dir(job_dir, &job.install)?;
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent) };
    ensure!(!handle.is_null(), "无法等待主程序退出");
    struct Handle(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    let handle = Handle(handle);
    fs::write(job_dir.join("ready"), b"ready")?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if job_dir.join("cancel").exists() {
            return Ok(());
        }
        match unsafe { WaitForSingleObject(handle.0, 100) } {
            WAIT_OBJECT_0 => break,
            WAIT_TIMEOUT => ensure!(
                Instant::now() < deadline,
                "等待主程序退出超时，尚未替换文件"
            ),
            _ => bail!("等待主程序失败"),
        }
    }
    let lock = installation_lock(&job.install, true)
        .context("另一个帧库实例正在使用此程序目录，请关闭后重试更新")?;
    let result = install(job_dir, &job, || {
        // Record before spawning: the child can read its result immediately.
        fs::write(
            job_dir.join("result.txt"),
            format!("已更新至 {}", job.version),
        )?;
        restart(&job, job_dir)
    });
    if let Err(error) = result {
        let message = format!("{error:#}");
        fs::write(job_dir.join("result.txt"), &message)?;
        lock.unlock()?;
        if !job_dir.join("recovery-required").exists() && restart(&job, job_dir).is_ok() {
            return Ok(());
        }
        bail!("{message}\n更新任务保留在：{}", job_dir.display());
    }
    Ok(())
}
fn restart(job: &Job, job_dir: &Path) -> Result<()> {
    Command::new(job.install.join(MAIN_EXE))
        .arg("--update-result")
        .arg(job_dir)
        .env("VIDEO_MANAGER_DATA_DIR", &job.data)
        .current_dir(&job.install)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("无法启动程序")?;
    Ok(())
}
fn validate_job_dir(directory: &Path, install: &Path) -> Result<()> {
    let root = dunce::canonicalize(install)?;
    let dir = dunce::canonicalize(directory)?;
    ensure!(
        dir.parent() == Some(root.as_path())
            && dir
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(JOB_PREFIX)),
        "更新任务目录不合法"
    );
    no_reparse(directory)?;
    Ok(())
}
pub fn cleanup_job(directory: &Path, install: &Path) -> Result<()> {
    validate_job_dir(directory, install)?;
    ensure!(
        !directory.join("recovery-required").exists(),
        "需保留恢复备份"
    );
    let job: Job = read_json(&directory.join("job.json"))?;
    let temp = dunce::canonicalize(std::env::temp_dir())?;
    if job.helper.exists() {
        let helper = dunce::canonicalize(&job.helper)?;
        ensure!(
            helper.parent() == Some(temp.as_path())
                && helper
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(HELPER_PREFIX)),
            "更新器临时目录不合法"
        );
        no_reparse(&job.helper)?;
        // May still be exiting after launching the app. Retain the job for a later retry.
        fs::remove_dir_all(&job.helper)?;
    }
    fs::remove_dir_all(directory)?;
    Ok(())
}
pub fn startup_result(root: &Path) -> Option<String> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().is_some_and(|a| a == "--update-result") {
        arguments.next().and_then(|p| {
            let path = PathBuf::from(p);
            validate_job_dir(&path, root).ok()?;
            fs::read_to_string(path.join("result.txt")).ok()
        })
    } else {
        None
    }
}
pub fn cleanup_completed(root: &Path) {
    // Only completed jobs are eligible; interrupted jobs and recovery backups stay intact.
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(JOB_PREFIX)
                && entry.path().join("result.txt").exists()
            {
                let _ = cleanup_job(&entry.path(), root);
            }
        }
    }
}
