#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod ui;
use std::path::{Path, PathBuf};

fn main() -> iced::Result {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--version")
    {
        println!("{}", video_management::update::VERSION);
        return Ok(());
    }
    let executable =
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("video-management.exe"));
    let root = executable.parent().unwrap_or(Path::new(".")).to_path_buf();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let _installation_lock = loop {
        match video_management::update::installation_lock(&root, false) {
            Ok(lock) => break lock,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100))
            }
            Err(error) => {
                video_management::update::show_error(&format!(
                    "程序目录正在更新或不可写，请稍后重试：{error:#}"
                ));
                return Ok(());
            }
        }
    };
    let update_result = video_management::update::startup_result(&root);
    let data = std::env::var_os("VIDEO_MANAGER_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("data"));
    let setup_error = configure_runtime(&root, &data).err();
    let gst_error = if setup_error.is_none() {
        gstreamer::init()
            .err()
            .map(|e| format!("播放组件初始化失败：{e}"))
    } else {
        None
    };
    let mut app = iced::daemon(
        move || {
            ui::App::boot(
                data.clone(),
                setup_error.clone().or(gst_error.clone()),
                update_result.clone(),
            )
        },
        ui::App::update,
        ui::App::view,
    )
    .title(ui::App::title)
    .subscription(ui::App::subscription)
    .theme(ui::App::theme)
    .style(|_, theme| iced::theme::Style {
        background_color: iced::Color::TRANSPARENT,
        text_color: theme.palette().text,
    })
    .default_font(ui::fluent::TEXT_FONT)
    .font(ui::fluent::TEXT_FONT_REGULAR)
    .font(ui::fluent::TEXT_FONT_BOLD);
    let fonts = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
        .join("Fonts");
    if let Ok(font) = std::fs::read(fonts.join("SegoeIcons.ttf")) {
        app = app.font(font);
    }
    let cleanup = std::thread::spawn(move || {
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            video_management::update::cleanup_completed(&root);
        }
    });
    let result = app.run();
    let _ = cleanup.join();
    result
}

fn configure_runtime(root: &Path, data: &Path) -> Result<(), String> {
    std::fs::create_dir_all(data).map_err(|e| {
        format!(
            "无法创建数据目录 {}：{e}。请将软件放到可写目录。",
            data.display()
        )
    })?;
    let cache = data.join("cache");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    // This runs before iced, GStreamer, and all application threads. No concurrent environment readers exist.
    unsafe {
        // D3D12 + DirectComposition preserves alpha for the native DWM backdrop.
        // An explicit developer backend override is respected; unsupported
        // surface alpha modes are handled by the opaque background fallback.
        if std::env::var_os("WGPU_BACKEND").is_none() {
            std::env::set_var("WGPU_BACKEND", "dx12");
        }
        if std::env::var_os("WGPU_DX12_PRESENTATION_SYSTEM").is_none() {
            std::env::set_var("WGPU_DX12_PRESENTATION_SYSTEM", "Visual");
        }
        std::env::set_var("GST_REGISTRY_1_0", cache.join("gstreamer-registry.bin"));
        if root.join("lib/gstreamer-1.0").is_dir() {
            std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", root.join("lib/gstreamer-1.0"));
            std::env::set_var("GST_PLUGIN_PATH_1_0", root.join("lib/gstreamer-1.0"));
            std::env::set_var(
                "GST_PLUGIN_SCANNER_1_0",
                root.join("libexec/gstreamer-1.0/gst-plugin-scanner.exe"),
            );
            let mut paths = vec![root.to_path_buf()];
            if let Some(old) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&old));
            }
            std::env::set_var(
                "PATH",
                std::env::join_paths(paths).map_err(|e| e.to_string())?,
            );
        }
    }
    Ok(())
}
