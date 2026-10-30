use rusqlite::{Connection, params};
use std::path::Path;
use video_management::{database::Database, model::*};

fn legacy_database(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE groups (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(trim(name))>0));
        CREATE TABLE videos (
            id INTEGER PRIMARY KEY, hash TEXT NOT NULL UNIQUE CHECK(length(hash)=64),
            title TEXT NOT NULL, group_id INTEGER REFERENCES groups(id) ON DELETE SET NULL,
            library INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
        CREATE TABLE files (
            id INTEGER PRIMARY KEY, video_id INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
            path TEXT NOT NULL, path_key TEXT NOT NULL UNIQUE, size INTEGER NOT NULL, modified INTEGER NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('pending','managed','ignored')),
            availability TEXT NOT NULL CHECK(availability IN ('present','missing','inaccessible')));
        CREATE INDEX files_video ON files(video_id);
        CREATE INDEX files_state ON files(state);
        CREATE TABLE roots (path_key TEXT PRIMARY KEY, path TEXT NOT NULL);
        CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO groups VALUES(7,'旅行'),(12,'空分组');
        INSERT INTO roots VALUES('c:\\videos','C:\\Videos');
        INSERT INTO settings VALUES('player','{\"volume\":0.33,\"muted\":true}');
        PRAGMA user_version=1;",
    ).unwrap();
    for (id, group, library, state, availability) in [
        (1, Some(7), true, "managed", "present"),
        (2, Some(7), false, "pending", "missing"),
        (3, Some(7), false, "ignored", "inaccessible"),
        (4, None, true, "managed", "present"),
    ] {
        conn.execute(
            "INSERT INTO videos VALUES(?,?,?,?,?,?)",
            params![
                id,
                format!("{id:064x}"),
                format!("视频{id}"),
                group,
                library,
                id * 100
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files VALUES(?,?,?,?,?,?,?,?)",
            params![
                id,
                id,
                format!("C:\\Videos\\{id}.mp4"),
                format!("c:\\videos\\{id}.mp4"),
                id * 10,
                id * 1000,
                state,
                availability
            ],
        )
        .unwrap();
    }
    conn.execute_batch("INSERT INTO files SELECT 5,video_id,'C:\\Videos\\copy.mp4','c:\\videos\\copy.mp4',size,modified,state,availability FROM files WHERE id=1;").unwrap();
    conn
}

fn observe(db: &mut Database, id: i64) -> i64 {
    db.observe(&Fingerprint {
        path: format!("C:\\Videos\\{id}.mp4").into(),
        size: 10,
        modified: 100,
        hash: format!("{id:064x}"),
        explicit: true,
    })
    .unwrap();
    db.snapshot()
        .unwrap()
        .videos
        .iter()
        .find(|v| v.hash == format!("{id:064x}"))
        .unwrap()
        .id
}

fn tags(db: &Database, id: i64) -> Vec<i64> {
    db.snapshot()
        .unwrap()
        .videos
        .iter()
        .find(|v| v.id == id)
        .unwrap()
        .tag_ids
        .clone()
}

#[test]
fn migration_preserves_all_states_empty_groups_and_other_data_and_runs_once() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("library.sqlite3");
    drop(legacy_database(&path));
    let mut db = Database::open(&path).unwrap();
    let s = db.snapshot().unwrap();
    assert_eq!(s.tags.len(), 2);
    assert!(s.tags.iter().any(|t| t.id == 12 && t.name == "空分组"));
    assert_eq!(tags(&db, 1), vec![7]);
    assert_eq!(tags(&db, 2), vec![7]);
    assert!(tags(&db, 4).is_empty());
    assert!(!s.videos.iter().any(|v| v.id == 3));
    assert_eq!(s.videos.iter().find(|v| v.id == 1).unwrap().files.len(), 2);
    assert_eq!(s.roots, vec![std::path::PathBuf::from("C:\\Videos")]);
    assert_eq!(db.settings().unwrap().volume, 0.33);
    assert!(db.settings().unwrap().muted);
    let files = db.files().unwrap();
    for (i, f) in files.iter().take(4).enumerate() {
        assert_eq!(f.size, (i as u64 + 1) * 10);
        assert_eq!(f.modified, (i as i64 + 1) * 1000);
        assert_eq!(
            f.path,
            std::path::PathBuf::from(format!("C:\\Videos\\{}.mp4", i + 1))
        );
    }
    assert_eq!(files[1].availability, Availability::Missing);
    assert_eq!(files[2].state, FileState::Ignored);
    assert_eq!(files[2].availability, Availability::Inaccessible);
    // Explicit reimport makes the previously hidden content visible with its migrated tag.
    assert_eq!(observe(&mut db, 3), 3);
    assert_eq!(tags(&db, 3), vec![7]);
    db.assign(&[1], TagAssignment::Add(vec![12]), false)
        .unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(tags(&db, 1), vec![7, 12]);
    assert_eq!(db.snapshot().unwrap().tags.len(), 2);
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='groups'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM pragma_table_info('videos') WHERE name='group_id'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
}

#[test]
fn failed_migration_rolls_back_schema_data_and_version() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("library.sqlite3");
    let conn = legacy_database(&path);
    conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
    conn.execute("UPDATE videos SET group_id=999 WHERE id=3", [])
        .unwrap();
    drop(conn);
    assert!(Database::open(&path).is_err());
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM groups", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row("SELECT group_id FROM videos WHERE id=3", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        999
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN ('tags','video_tags')",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
}

#[test]
fn batch_add_remove_preserves_individual_tags_and_accepts_atomically() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("library.sqlite3");
    let mut db = Database::open(&path).unwrap();
    let a = observe(&mut db, 1);
    let b = observe(&mut db, 2);
    for name in ["a", "b", "c"] {
        db.create_tag(name).unwrap();
    }
    let ids: Vec<_> = db.snapshot().unwrap().tags.iter().map(|t| t.id).collect();
    let [x, y, z] = ids.as_slice() else { panic!() };
    db.assign(&[a], TagAssignment::Add(vec![*x]), false)
        .unwrap();
    db.assign(&[b], TagAssignment::Add(vec![*y]), false)
        .unwrap();
    db.assign(&[a, b], TagAssignment::Add(vec![*z, *z]), true)
        .unwrap();
    db.assign(&[a, b], TagAssignment::Add(vec![*z]), false)
        .unwrap();
    assert_eq!(tags(&db, a), vec![*x, *z]);
    assert_eq!(tags(&db, b), vec![*y, *z]);
    assert!(
        db.snapshot()
            .unwrap()
            .videos
            .iter()
            .all(|v| v.library && !v.pending())
    );
    db.assign(&[a, b], TagAssignment::Remove(vec![*x, *z, *z]), false)
        .unwrap();
    assert!(tags(&db, a).is_empty());
    assert_eq!(tags(&db, b), vec![*y]);
    db.assign(&[a, b], TagAssignment::Keep, true).unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    assert!(tags(&db, a).is_empty());
    assert_eq!(tags(&db, b), vec![*y]);
}

#[test]
fn invalid_tag_or_video_rolls_back_entire_batch_and_pending_acceptance() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = Database::open(&tmp.path().join("library.sqlite3")).unwrap();
    let a = observe(&mut db, 1);
    let b = observe(&mut db, 2);
    db.create_tag("keep").unwrap();
    let tag = db.snapshot().unwrap().tags[0].id;
    db.assign(&[a], TagAssignment::Add(vec![tag]), false)
        .unwrap();
    for assignment in [
        TagAssignment::Add(vec![tag, 999]),
        TagAssignment::Remove(vec![tag, 999]),
    ] {
        assert!(db.assign(&[a, b], assignment, true).is_err());
        assert_eq!(tags(&db, a), vec![tag]);
        assert!(tags(&db, b).is_empty());
        assert!(
            db.snapshot()
                .unwrap()
                .videos
                .iter()
                .all(|v| !v.library && v.pending())
        );
    }
    assert!(
        db.assign(&[a, b, 999], TagAssignment::Remove(vec![tag]), true)
            .is_err()
    );
    assert_eq!(tags(&db, a), vec![tag]);
    assert!(
        db.snapshot()
            .unwrap()
            .videos
            .iter()
            .all(|v| !v.library && v.pending())
    );
}

#[test]
fn deleting_tag_preserves_other_tags_and_name_validation_is_consistent() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = Database::open(&tmp.path().join("library.sqlite3")).unwrap();
    let a = observe(&mut db, 1);
    db.create_tag("  Travel  ").unwrap();
    assert!(db.create_tag("travel").is_err());
    assert!(db.create_tag(" \t ").is_err());
    assert!(db.create_tag(&"中".repeat(81)).is_err());
    db.create_tag(&format!(" {} ", "中".repeat(80))).unwrap();
    let ids: Vec<_> = db.snapshot().unwrap().tags.iter().map(|t| t.id).collect();
    db.assign(&[a], TagAssignment::Add(ids.clone()), true)
        .unwrap();
    assert!(db.rename_tag(ids[1], "TRAVEL").is_err());
    db.rename_tag(ids[1], "  收藏  ").unwrap();
    db.delete_tag(ids[0]).unwrap();
    assert_eq!(tags(&db, a), vec![ids[1]]);
    assert_eq!(db.snapshot().unwrap().tags[0].name, "收藏");
    assert!(db.snapshot().unwrap().videos[0].library);
    assert_eq!(db.files().unwrap().len(), 1);
    db.delete_tag(ids[1]).unwrap();
    assert!(tags(&db, a).is_empty());
    let file_id = db.files().unwrap()[0].id;
    db.remove_file(file_id).unwrap();
    assert!(db.snapshot().unwrap().videos.is_empty());
}
