use std::time::Duration;
use video_management::{
    database::Database,
    model::{AppSettings, Settings, ThemePreference, UpdateSettings},
    service::{Command, Event, Service},
};

#[test]
fn concurrency_limit_matches_available_cpu_and_default_is_two() {
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    assert_eq!(AppSettings::max_concurrency(), available);
    assert_eq!(AppSettings::default().concurrency, 2.min(available));
}

#[test]
fn application_settings_preserve_legacy_data_and_validate_saved_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    let db = Database::open(&path).unwrap();
    db.save_settings(&Settings {
        volume: 0.24,
        muted: true,
    })
    .unwrap();
    db.save_update_settings(&UpdateSettings {
        folder: Some(dir.path().join("更新")),
    })
    .unwrap();
    assert_eq!(db.app_settings().unwrap(), (AppSettings::default(), None));
    let desired = AppSettings {
        concurrency: AppSettings::max_concurrency(),
        theme: ThemePreference::Dark,
    };
    db.save_app_settings(&desired).unwrap();
    assert!(
        db.save_app_settings(&AppSettings {
            concurrency: 0,
            ..desired.clone()
        })
        .is_err()
    );
    assert!(
        db.save_app_settings(&AppSettings {
            concurrency: AppSettings::max_concurrency() + 1,
            ..desired.clone()
        })
        .is_err()
    );
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.app_settings().unwrap().0, desired);
    assert_eq!(db.settings().unwrap().volume, 0.24);
    assert!(db.settings().unwrap().muted);
    assert_eq!(
        db.update_settings().unwrap().folder,
        Some(dir.path().join("更新"))
    );
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute(
        "UPDATE settings SET value=? WHERE key='application'",
        [r#"{"theme":"System"}"#],
    )
    .unwrap();
    assert_eq!(
        db.app_settings().unwrap().0,
        AppSettings {
            theme: ThemePreference::System,
            ..AppSettings::default()
        }
    );
    raw.execute(
        "UPDATE settings SET value=? WHERE key='application'",
        [r#"{"concurrency":0,"theme":"Dark"}"#],
    )
    .unwrap();
    let (settings, warning) = db.app_settings().unwrap();
    assert_eq!(settings.concurrency, AppSettings::default().concurrency);
    assert_eq!(settings.theme, ThemePreference::Dark);
    assert!(warning.is_some());
    raw.execute(
        "UPDATE settings SET value=? WHERE key='application'",
        [serde_json::to_string(&AppSettings {
            concurrency: AppSettings::max_concurrency() + 1,
            theme: ThemePreference::Dark,
        })
        .unwrap()],
    )
    .unwrap();
    let (settings, warning) = db.app_settings().unwrap();
    assert_eq!(settings.concurrency, AppSettings::default().concurrency);
    assert_eq!(settings.theme, ThemePreference::Dark);
    assert!(
        warning
            .unwrap()
            .contains(&format!("1–{}", AppSettings::max_concurrency()))
    );
    raw.execute(
        "UPDATE settings SET value='broken' WHERE key='application'",
        [],
    )
    .unwrap();
    assert_eq!(db.app_settings().unwrap().0, AppSettings::default());
    assert!(db.app_settings().unwrap().1.is_some());
}

fn loaded(service: &Service) {
    while !matches!(
        service.events.recv_timeout(Duration::from_secs(5)).unwrap(),
        Event::AppSettings(_)
    ) {}
}
#[test]
fn ordinary_and_update_shutdown_flush_application_settings() {
    for update in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::start(dir.path().to_path_buf()).unwrap();
        loaded(&service);
        let desired = AppSettings {
            concurrency: 3.min(AppSettings::max_concurrency()),
            theme: ThemePreference::System,
        };
        service.send(Command::AppSettings(desired.clone()));
        if update {
            service.shutdown_for_update().unwrap();
        } else {
            service.shutdown();
        }
        assert!(
            service
                .events
                .try_iter()
                .any(|e| matches!(e, Event::AppSettingsSaved(Ok(s)) if s == desired))
        );
        let db = Database::open(&dir.path().join("library.sqlite3")).unwrap();
        assert_eq!(db.app_settings().unwrap().0, desired);
    }
}

#[test]
fn failed_save_is_reported_and_previous_settings_remain_effective() {
    let dir = tempfile::tempdir().unwrap();
    let mut service = Service::start(dir.path().to_path_buf()).unwrap();
    loaded(&service);
    let db = rusqlite::Connection::open(dir.path().join("library.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_settings BEFORE INSERT ON settings WHEN NEW.key='application' BEGIN SELECT RAISE(FAIL, 'test write failure'); END;").unwrap();
    service.send(Command::AppSettings(AppSettings {
        concurrency: 8.min(AppSettings::max_concurrency()),
        theme: ThemePreference::Dark,
    }));
    loop {
        if let Event::AppSettingsSaved(result) =
            service.events.recv_timeout(Duration::from_secs(5)).unwrap()
        {
            assert!(result.unwrap_err().contains("test write failure"));
            break;
        }
    }
    service.shutdown();
    assert_eq!(
        Database::open(&dir.path().join("library.sqlite3"))
            .unwrap()
            .app_settings()
            .unwrap()
            .0,
        AppSettings::default()
    );
}
