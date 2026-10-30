use crate::model::*;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Database {
    conn: Connection,
}

pub fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path).context("无法打开数据库，请检查 data 目录权限")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;",
        )?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 3 {
            bail!("数据库由较新版本创建，请使用较新版本的软件");
        }
        if version == 0 {
            conn.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE groups (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(trim(name))>0));
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
                PRAGMA user_version=1; COMMIT;")?;
        }
        if version < 2 {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute_batch(
                "CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(trim(name))>0));
                CREATE TABLE video_tags (
                    video_id INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
                    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                    PRIMARY KEY(video_id,tag_id));
                CREATE INDEX video_tags_tag ON video_tags(tag_id);
                INSERT INTO tags(id,name) SELECT id,name FROM groups;
                INSERT INTO video_tags(video_id,tag_id) SELECT id,group_id FROM videos WHERE group_id IS NOT NULL;
                ALTER TABLE videos DROP COLUMN group_id;
                DROP TABLE groups;
                PRAGMA user_version=2;",
            ).context("无法将分组迁移为标签，原有数据已保留")?;
            tx.commit()?;
        }
        if version < 3 {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute_batch(
                "ALTER TABLE videos ADD COLUMN duration_ms INTEGER CHECK(duration_ms > 0);
                ALTER TABLE videos ADD COLUMN duration_checked INTEGER NOT NULL DEFAULT 0;
                PRAGMA user_version=3;",
            )?;
            tx.commit()?;
        }
        Ok(Self { conn })
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut s = Snapshot::default();
        let mut q = self
            .conn
            .prepare("SELECT id,name FROM tags ORDER BY name COLLATE NOCASE,id")?;
        s.tags = q
            .query_map([], |r| {
                Ok(Tag {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut q = self.conn.prepare("SELECT path FROM roots ORDER BY path")?;
        s.roots = q
            .query_map([], |r| Ok(PathBuf::from(r.get::<_, String>(0)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut q = self.conn.prepare("SELECT id,hash,title,library,created_at,duration_ms FROM videos WHERE library=1 OR EXISTS(SELECT 1 FROM files WHERE video_id=videos.id AND state='pending') ORDER BY created_at DESC,id DESC")?;
        s.videos = q
            .query_map([], |r| {
                Ok(VideoItem {
                    id: r.get(0)?,
                    hash: r.get(1)?,
                    title: r.get(2)?,
                    tag_ids: Vec::new(),
                    library: r.get(3)?,
                    created_at: r.get(4)?,
                    duration_ms: r
                        .get::<_, Option<i64>>(5)?
                        .filter(|ms| *ms > 0)
                        .map(|ms| ms as u64),
                    files: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let index: HashMap<i64, usize> = s
            .videos
            .iter()
            .enumerate()
            .map(|(i, v)| (v.id, i))
            .collect();
        let mut q = self
            .conn
            .prepare("SELECT video_id,tag_id FROM video_tags ORDER BY video_id,tag_id")?;
        for association in q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (video, tag) = association?;
            if let Some(&i) = index.get(&video) {
                s.videos[i].tag_ids.push(tag);
            }
        }
        for file in self.files()? {
            if file.state != FileState::Ignored
                && let Some(&i) = index.get(&file.video_id)
            {
                s.videos[i].files.push(file);
            }
        }
        Ok(s)
    }

    pub fn duration_status(&self, retry_unknown: bool) -> Result<BTreeMap<i64, DurationStatus>> {
        let mut q = self.conn.prepare(
            "SELECT id,hash,duration_checked AND (?=0 OR duration_ms IS NOT NULL) FROM videos",
        )?;
        Ok(q.query_map([retry_unknown], |r| {
            Ok((
                r.get(0)?,
                DurationStatus {
                    hash: r.get(1)?,
                    checked: r.get(2)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_duration(&self, hash: &str, duration_ms: Option<u64>) -> Result<()> {
        // Content identity keeps late results away from a replaced file. Failed
        // reads never overwrite a successful result from another copy.
        let duration_ms = duration_ms
            .filter(|ms| *ms > 0)
            .map(i64::try_from)
            .transpose()?;
        self.conn.execute(
            "UPDATE videos SET duration_ms=COALESCE(duration_ms,?),duration_checked=1 WHERE hash=?",
            params![duration_ms, hash],
        )?;
        Ok(())
    }

    pub fn files(&self) -> Result<Vec<VideoFile>> {
        let mut q = self.conn.prepare(
            "SELECT id,video_id,path,size,modified,state,availability FROM files ORDER BY id",
        )?;
        Ok(q.query_map([], |r| {
            Ok(VideoFile {
                id: r.get(0)?,
                video_id: r.get(1)?,
                path: PathBuf::from(r.get::<_, String>(2)?),
                size: r.get::<_, i64>(3)? as u64,
                modified: r.get(4)?,
                state: FileState::parse(&r.get::<_, String>(5)?),
                availability: Availability::parse(&r.get::<_, String>(6)?),
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
    }

    /// Returns true only when this observation creates a new pending file.
    pub fn observe(&mut self, fingerprint: &Fingerprint) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let key = path_key(&fingerprint.path);
        let old: Option<(i64, String)> = tx
            .query_row(
                "SELECT video_id,state FROM files WHERE path_key=?",
                [&key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let title = fingerprint
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        tx.execute(
            "INSERT OR IGNORE INTO videos(hash,title,created_at) VALUES(?,?,?)",
            params![fingerprint.hash, title, now],
        )?;
        let (video_id, library): (i64, bool) = tx.query_row(
            "SELECT id,library FROM videos WHERE hash=?",
            [&fingerprint.hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let state = if let Some((old_video, old_state)) = &old {
            if *old_video == video_id {
                if old_state == "ignored" && fingerprint.explicit {
                    "pending"
                } else {
                    old_state
                }
            } else {
                "pending"
            }
        } else if library && !fingerprint.explicit {
            let (present,missing):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(availability='present'),0),COALESCE(SUM(availability='missing'),0) FROM files WHERE video_id=? AND state='managed'",[video_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if present == 0 && missing > 0 {
                "managed"
            } else {
                "pending"
            }
        } else {
            "pending"
        };
        let is_new = state == "pending"
            && !old
                .as_ref()
                .is_some_and(|(v, s)| *v == video_id && s == "pending");
        tx.execute("INSERT INTO files(video_id,path,path_key,size,modified,state,availability) VALUES(?,?,?,?,?,?,'present')
            ON CONFLICT(path_key) DO UPDATE SET video_id=excluded.video_id,path=excluded.path,size=excluded.size,modified=excluded.modified,state=excluded.state,availability='present'",
            params![video_id,fingerprint.path.to_string_lossy(),key,fingerprint.size as i64,fingerprint.modified,state])?;
        tx.execute("DELETE FROM videos WHERE library=0 AND NOT EXISTS(SELECT 1 FROM files WHERE video_id=videos.id)",[])?;
        tx.commit()?;
        Ok(is_new)
    }

    pub fn availability(&self, path: &Path, availability: Availability) -> Result<()> {
        self.conn.execute(
            "UPDATE files SET availability=? WHERE path_key=?",
            params![availability.as_str(), path_key(path)],
        )?;
        Ok(())
    }

    pub fn create_tag(&self, name: &str) -> Result<()> {
        validate_tag(name)?;
        self.conn
            .execute("INSERT INTO tags(name) VALUES(?)", [name.trim()])
            .context("标签名称已存在")?;
        Ok(())
    }
    pub fn rename_tag(&self, id: i64, name: &str) -> Result<()> {
        validate_tag(name)?;
        self.conn
            .execute(
                "UPDATE tags SET name=? WHERE id=?",
                params![name.trim(), id],
            )
            .context("标签名称已存在")?;
        Ok(())
    }
    pub fn delete_tag(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM tags WHERE id=?", [id])?;
        Ok(())
    }

    pub fn assign(&mut self, ids: &[i64], assignment: TagAssignment, accept: bool) -> Result<()> {
        let tx = self.conn.transaction()?;
        let tags = match &assignment {
            TagAssignment::Keep => &[][..],
            TagAssignment::Add(tags) | TagAssignment::Remove(tags) => tags.as_slice(),
        };
        for tag in tags {
            if !tx.query_row("SELECT EXISTS(SELECT 1 FROM tags WHERE id=?)", [tag], |r| {
                r.get::<_, bool>(0)
            })? {
                bail!("标签已不存在，请重新选择");
            }
        }
        for id in ids {
            // A stale selection must not partially change other videos or accept pending files.
            if !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM videos WHERE id=?)",
                [id],
                |r| r.get::<_, bool>(0),
            )? {
                bail!("视频已不存在，请重新选择");
            }
            for tag in tags {
                match &assignment {
                    TagAssignment::Add(_) => {
                        tx.execute("INSERT INTO video_tags(video_id,tag_id) VALUES(?,?) ON CONFLICT(video_id,tag_id) DO NOTHING", params![id,tag])?;
                    }
                    TagAssignment::Remove(_) => {
                        tx.execute(
                            "DELETE FROM video_tags WHERE video_id=? AND tag_id=?",
                            params![id, tag],
                        )?;
                    }
                    TagAssignment::Keep => {}
                }
            }
            if accept {
                tx.execute(
                    "UPDATE files SET state='managed' WHERE video_id=? AND state='pending'",
                    [id],
                )?;
                tx.execute("UPDATE videos SET library=1 WHERE id=? AND EXISTS(SELECT 1 FROM files WHERE video_id=? AND state='managed')",params![id,id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn forget(&mut self, ids: &[i64], pending_only: bool) -> Result<()> {
        let tx = self.conn.transaction()?;
        for id in ids {
            if pending_only {
                tx.execute(
                    "UPDATE files SET state='ignored' WHERE video_id=? AND state='pending'",
                    [id],
                )?;
            } else {
                tx.execute("UPDATE files SET state='ignored' WHERE video_id=?", [id])?;
                tx.execute("UPDATE videos SET library=0 WHERE id=?", [id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_file(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let video: Option<i64> = tx
            .query_row("SELECT video_id FROM files WHERE id=?", [id], |r| r.get(0))
            .optional()?;
        tx.execute("DELETE FROM files WHERE id=?", [id])?;
        if let Some(video) = video {
            // Historical missing locations should not keep an intentionally deleted item alive.
            let live:i64=tx.query_row("SELECT count(*) FROM files WHERE video_id=? AND state!='ignored' AND availability!='missing'",[video],|r|r.get(0))?;
            if live == 0 {
                tx.execute("UPDATE videos SET library=0 WHERE id=?", [video])?;
                tx.execute(
                    "DELETE FROM files WHERE video_id=? AND state!='ignored'",
                    [video],
                )?;
            }
            tx.execute("DELETE FROM videos WHERE id=? AND NOT EXISTS(SELECT 1 FROM files WHERE video_id=?)",params![video,video])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn matches_delete_target(&self, target: &DeleteTarget) -> Result<bool> {
        Ok(self.conn.query_row("SELECT EXISTS(SELECT 1 FROM files JOIN videos ON videos.id=files.video_id WHERE files.id=? AND files.path_key=? AND videos.hash=?)",params![target.file_id,path_key(&target.path),target.hash],|r|r.get(0))?)
    }

    pub fn add_root(&self, path: &Path) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO roots(path_key,path) VALUES(?,?)",
            params![path_key(path), path.to_string_lossy()],
        )?;
        Ok(())
    }
    pub fn remove_root(&self, path: &Path) -> Result<()> {
        self.conn
            .execute("DELETE FROM roots WHERE path_key=?", [path_key(path)])?;
        Ok(())
    }
    pub fn settings(&self) -> Result<Settings> {
        let value: Option<String> = self
            .conn
            .query_row("SELECT value FROM settings WHERE key='player'", [], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(value
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default())
    }
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.conn.execute("INSERT INTO settings(key,value) VALUES('player',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(settings)?])?;
        Ok(())
    }

    pub fn app_settings(&self) -> Result<(AppSettings, Option<String>)> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key='application'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let mut settings: AppSettings = match value {
            Some(value) => match serde_json::from_str(&value) {
                Ok(settings) => settings,
                Err(error) => {
                    return Ok((
                        AppSettings::default(),
                        Some(format!("应用设置无法读取，已使用默认值：{error}")),
                    ));
                }
            },
            None => AppSettings::default(),
        };
        let max = AppSettings::max_concurrency();
        let warning = if !(1..=max).contains(&settings.concurrency) {
            settings.concurrency = AppSettings::default().concurrency;
            Some(format!("并发线程数超出当前范围 1–{max}，已恢复默认值"))
        } else {
            None
        };
        Ok((settings, warning))
    }
    pub fn save_app_settings(&self, settings: &AppSettings) -> Result<()> {
        let max = AppSettings::max_concurrency();
        anyhow::ensure!(
            (1..=max).contains(&settings.concurrency),
            "并发线程数必须为 1–{max}"
        );
        self.conn.execute("INSERT INTO settings(key,value) VALUES('application',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(settings)?])?;
        Ok(())
    }

    pub fn update_settings(&self) -> Result<UpdateSettings> {
        let value: Option<String> = self
            .conn
            .query_row("SELECT value FROM settings WHERE key='updates'", [], |r| {
                r.get(0)
            })
            .optional()?;
        value
            .map(|s| serde_json::from_str(&s).context("更新设置无法读取"))
            .unwrap_or_else(|| Ok(UpdateSettings::default()))
    }

    pub fn save_update_settings(&self, settings: &UpdateSettings) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key,value) VALUES('updates',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(settings)?],
        )?;
        Ok(())
    }
}

fn validate_tag(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.trim().chars().count() > 80 {
        bail!("标签名称必须为 1–80 个字符");
    }
    Ok(())
}
