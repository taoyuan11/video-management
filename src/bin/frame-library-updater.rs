#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--version")
    {
        println!("{}", video_management::update::VERSION);
        return;
    }
    let result = (|| -> anyhow::Result<()> {
        let mut args = std::env::args_os().skip(1);
        anyhow::ensure!(
            args.next().is_some_and(|a| a == "--apply"),
            "请从帧库的软件更新入口运行更新器"
        );
        let directory = args
            .next()
            .map(std::path::PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("缺少更新任务"))?;
        let pid = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("缺少主程序标识"))?
            .to_string_lossy()
            .parse()?;
        video_management::update::run_helper(&directory, pid)
    })();
    if let Err(error) = result {
        video_management::update::show_error(&format!("{error:#}"));
        std::process::exit(1);
    }
}
