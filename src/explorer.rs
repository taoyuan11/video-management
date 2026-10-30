//! Windows Shell selection, called on the service thread so disk/network access
//! and Shell initialization do not block the UI thread.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf, Prefix},
    ptr,
};
use windows_sys::Win32::{
    System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
    UI::Shell::{Common::ITEMIDLIST, SHOpenFolderAndSelectItems, SHParseDisplayName},
};

pub(crate) fn open(path: &Path) -> Result<()> {
    let result = (|| {
        let selection = FileSelection::new(path)?;
        // With cidl = 0, the absolute PIDL identifies the file to select in its
        // parent folder. No command-line quoting or file association is involved.
        check(
            unsafe { SHOpenFolderAndSelectItems(selection.item, 0, ptr::null(), 0) },
            "资源管理器未能定位文件",
        )
    })();
    result.with_context(|| format!("无法在资源管理器中打开：{}", path.display()))
}

struct ComApartment;

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct FileSelection {
    item: *mut ITEMIDLIST,
    _apartment: ComApartment,
}

impl FileSelection {
    fn new(path: &Path) -> Result<Self> {
        let wide = shell_path(path)?;
        let metadata = std::fs::metadata(path).context("文件已移动、删除或暂时无法访问")?;
        ensure!(metadata.is_file(), "该位置已不是视频文件");
        // Removing a verbatim prefix can change names ending in dots/spaces.
        // Reject such a mismatch instead of silently selecting another file.
        let shell_path = PathBuf::from(OsString::from_wide(&wide[..wide.len() - 1]));
        ensure!(
            std::fs::canonicalize(&shell_path).context("资源管理器无法访问此路径")?
                == std::fs::canonicalize(path).context("文件暂时无法访问")?,
            "资源管理器无法准确定位此路径"
        );

        check(
            unsafe { CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32) },
            "无法初始化资源管理器组件",
        )?;
        // Both S_OK and S_FALSE require a matching CoUninitialize. The PIDL is
        // freed before this apartment is released, including on error paths.
        let mut selection = Self {
            item: ptr::null_mut(),
            _apartment: ComApartment,
        };
        check(
            unsafe {
                SHParseDisplayName(
                    wide.as_ptr(),
                    ptr::null_mut(),
                    &mut selection.item,
                    0,
                    ptr::null_mut(),
                )
            },
            "资源管理器无法识别文件路径",
        )?;
        ensure!(!selection.item.is_null(), "资源管理器未返回文件位置");
        Ok(selection)
    }
}

impl Drop for FileSelection {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(self.item.cast()) };
    }
}

fn shell_path(path: &Path) -> Result<Vec<u16>> {
    ensure!(path.is_absolute(), "视频路径必须为绝对路径");
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    ensure!(!wide.contains(&0), "视频路径包含无效字符");
    // Shell parsing accepts long DOS/UNC names, but not the extended Win32
    // namespace returned by canonicalization (including NAS paths).
    if let Some(Component::Prefix(prefix)) = path.components().next() {
        match prefix.kind() {
            Prefix::VerbatimUNC(..) => {
                wide.drain(2..8);
            }
            Prefix::VerbatimDisk(_) => {
                wide.drain(..4);
            }
            _ => {}
        }
    }
    wide.push(0);
    Ok(wide)
}

fn check(result: i32, action: &str) -> Result<()> {
    ensure!(result >= 0, "{action}（HRESULT 0x{:08X}）", result as u32);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Shell::SHGetPathFromIDListEx;

    fn assert_shell_target(path: &Path) {
        // Resolve the actual Shell identifier back to a path without opening a
        // window or calling SHOpenFolderAndSelectItems during automated tests.
        let selection = FileSelection::new(path).unwrap();
        let mut buffer = vec![0u16; 32768];
        assert_ne!(
            unsafe {
                SHGetPathFromIDListEx(selection.item, buffer.as_mut_ptr(), buffer.len() as u32, 0)
            },
            0
        );
        let end = buffer.iter().position(|c| *c == 0).unwrap();
        let resolved = PathBuf::from(OsString::from_wide(&buffer[..end]));
        assert_eq!(
            std::fs::canonicalize(resolved).unwrap(),
            std::fs::canonicalize(path).unwrap()
        );
    }

    #[test]
    fn shell_selection_preserves_unicode_spaces_and_punctuation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("中文 空格 #100% & '副本',.mp4");
        std::fs::write(&path, b"video").unwrap();
        assert_shell_target(&path);
        assert_shell_target(&std::fs::canonicalize(&path).unwrap());
    }

    #[test]
    fn shell_selection_resolves_long_paths() {
        let temp = tempfile::tempdir().unwrap();
        let mut directory = std::fs::canonicalize(temp.path()).unwrap();
        for _ in 0..16 {
            directory.push("较长的中文目录 with spaces");
        }
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("视频.mp4");
        std::fs::write(&path, b"video").unwrap();
        assert!(path.as_os_str().encode_wide().count() > 260);
        assert_shell_target(&path);
    }

    #[test]
    fn unc_shell_paths_keep_the_server_share_and_filename() {
        let expected = Path::new(r"\\nas\共享目录\空 格 #100%,视频.mp4");
        for path in [
            expected,
            Path::new(r"\\?\UNC\nas\共享目录\空 格 #100%,视频.mp4"),
        ] {
            let wide = shell_path(path).unwrap();
            assert_eq!(wide.last(), Some(&0));
            assert_eq!(
                PathBuf::from(OsString::from_wide(&wide[..wide.len() - 1])),
                expected
            );
        }
    }

    #[test]
    fn invalid_or_unavailable_targets_fail_before_opening_explorer() {
        let temp = tempfile::tempdir().unwrap();
        for path in [
            PathBuf::from("video.mp4"),
            PathBuf::from(r"C:video.mp4"),
            temp.path().join("invalid\0.mp4"),
            temp.path().join("missing.mp4"),
            temp.path().to_path_buf(),
        ] {
            let error = open(&path).unwrap_err();
            assert!(format!("{error:#}").contains(&path.display().to_string()));
        }
    }
}
