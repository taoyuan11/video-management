use anyhow::{Context, Result};
use std::path::Path;
use url::Url;

/// Read container duration without decoding frames, playing audio or opening a window.
pub fn duration_ms(path: &Path, cancel: &std::sync::atomic::AtomicBool) -> Result<Option<u64>> {
    use gstreamer::{self as gst, prelude::*};
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };

    struct Pipeline(gst::Pipeline);
    impl Drop for Pipeline {
        fn drop(&mut self) {
            let _ = self.0.set_state(gst::State::Null);
        }
    }

    anyhow::ensure!(!cancel.load(Ordering::Relaxed), "时长读取已取消");
    gst::init()?;
    let pipeline = Pipeline(
        gst::parse::launch(
            "filesrc name=source ! parsebin ! fakesink sync=false enable-last-sample=false",
        )?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("无法创建时长读取管线"))?,
    );
    pipeline
        .0
        .by_name("source")
        .context("无法创建文件源")?
        .set_property("location", path.to_str().context("视频路径包含无效字符")?);
    let bus = pipeline.0.bus().context("无法读取媒体状态")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    pipeline.0.set_state(gst::State::Paused)?;
    loop {
        anyhow::ensure!(!cancel.load(Ordering::Relaxed), "时长读取已取消");
        anyhow::ensure!(Instant::now() < deadline, "时长读取超时");
        if let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(50)) {
            match message.view() {
                gst::MessageView::Error(error) => anyhow::bail!("{}", error.error()),
                gst::MessageView::AsyncDone(_) | gst::MessageView::Eos(_) => {
                    return Ok(pipeline
                        .0
                        .query_duration::<gst::ClockTime>()
                        .map(|time| time.mseconds())
                        .filter(|ms| *ms > 0));
                }
                _ => {}
            }
        }
    }
}

/// File URI accepted by GStreamer's Windows file source.
pub fn file_uri(path: &Path) -> Result<Url> {
    anyhow::ensure!(path.is_absolute(), "视频路径必须为绝对路径");
    let path = path.to_str().context("视频路径包含无效的 Unicode 字符")?;
    anyhow::ensure!(!path.contains('\0'), "视频路径包含无效字符");
    // filesrc rejects file://server/... . Keep the entire Windows path in the
    // URI path, encoding backslashes so url::Url cannot collapse leading UNC
    // separators or the verbatim prefix. GLib decodes it for filesrc to open.
    // Literal %, # and ? must also be escaped before parsing the URI.
    let escaped = gstreamer::glib::uri_escape_string(path.replace('/', "\\"), Some(":"), false);
    Url::parse(&format!("file:///{escaped}")).context("无法转换视频地址")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gstreamer::{self as gst, prelude::*};

    fn assert_file_source_path(path: &Path) {
        gst::init().unwrap();
        let uri = file_uri(path).unwrap();
        assert_eq!(uri.scheme(), "file");
        assert!(uri.host_str().is_none(), "remote authority: {uri}");
        assert!(uri.query().is_none());
        assert!(uri.fragment().is_none());
        let source = gst::ElementFactory::make("filesrc").build().unwrap();
        source
            .dynamic_cast_ref::<gst::URIHandler>()
            .unwrap()
            .set_uri(uri.as_str())
            .unwrap();
        // Read the actual path filesrc will open, not a second URL implementation.
        assert_eq!(Path::new(&source.property::<String>("location")), path);
    }

    #[test]
    fn local_paths_keep_unicode_and_reserved_uri_characters() {
        assert_file_source_path(Path::new(r#"C:\视频\空 格 #100% & '引号'.mp4"#));
        assert_file_source_path(Path::new(r"C:\视频\literal%20%23.mp4"));
        assert_file_source_path(Path::new("C:/视频/正斜杠.mp4"));
    }

    #[test]
    fn unc_paths_keep_server_and_share_in_the_file_path() {
        assert_file_source_path(Path::new(r"\\nas\nas\视频\测试.mp4"));
        assert_file_source_path(Path::new(r"\\192.0.2.1\共享目录\空 格 #100%.mp4"));
    }

    #[test]
    fn verbatim_paths_keep_the_windows_namespace() {
        assert_file_source_path(Path::new(r"\\?\UNC\nas\nas\视频\测试.mp4"));
        let long_path = format!(r"\\?\C:\{}视频.mp4", "长目录\\".repeat(80));
        assert_file_source_path(Path::new(&long_path));
    }

    #[test]
    fn relative_paths_are_rejected() {
        assert!(file_uri(Path::new("video.mp4")).is_err());
        assert!(file_uri(Path::new(r"C:video.mp4")).is_err());
    }
}
