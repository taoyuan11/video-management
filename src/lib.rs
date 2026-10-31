#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
compile_error!("video-management supports Windows 11 x64 only.");

pub mod database;
mod explorer;
#[cfg(feature = "gui")]
pub mod media;
pub mod migration;
pub mod model;
pub mod processing;
pub mod scanner;
pub mod service;
#[cfg(feature = "gui")]
pub mod thumbnails;
pub mod update;
