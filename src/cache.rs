use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use anyhow::Result;
use rusqlite::{params, Connection};
use crate::innertube::{PlaylistSummary, Track, UserProfile};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS liked_tracks (
                video_id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT NOT NULL,
                duration_secs INTEGER NOT NULL,
                thumbnail_url TEXT NOT NULL,
                added_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS play_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                video_id TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT NOT NULL,
                duration_secs INTEGER NOT NULL,
                thumbnail_url TEXT NOT NULL,
                played_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS stream_cache (
                video_id TEXT PRIMARY KEY,
                stream_url TEXT NOT NULL,
                expires_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS app_metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;

        // Ensure backward-compatible migration for playlists table
        let has_playlists = self.conn.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='playlists'")?.exists([])?;
        if has_playlists {
            let has_title = {
                let mut stmt = self.conn.prepare("PRAGMA table_info(playlists)")?;
                let mut rows = stmt.query([])?;
                let mut found = false;
                while let Some(row) = rows.next()? {
                    let col_name: String = row.get(1)?;
                    if col_name == "title" {
                        found = true;
                        break;
                    }
                }
                found
            };
            if !has_title {
                let count: i64 = self.conn.query_row("SELECT count(*) FROM playlists", [], |r| r.get(0)).unwrap_or(0);
                if count == 0 {
                    let _ = self.conn.execute("DROP TABLE playlists", []);
                }
            }
        }

        // Ensure backward-compatible migration for playlist_tracks table
        let has_playlist_tracks = self.conn.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='playlist_tracks'")?.exists([])?;
        if has_playlist_tracks {
            let has_duration_text = {
                let mut stmt = self.conn.prepare("PRAGMA table_info(playlist_tracks)")?;
                let mut rows = stmt.query([])?;
                let mut found = false;
                while let Some(row) = rows.next()? {
                    let col_name: String = row.get(1)?;
                    if col_name == "duration_text" {
                        found = true;
                        break;
                    }
                }
                found
            };
            if !has_duration_text {
                let count: i64 = self.conn.query_row("SELECT count(*) FROM playlist_tracks", [], |r| r.get(0)).unwrap_or(0);
                if count == 0 {
                    let _ = self.conn.execute("DROP TABLE playlist_tracks", []);
                }
            }
        }

        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS playlists (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                subtitle TEXT NOT NULL DEFAULT '',
                count_text TEXT NOT NULL DEFAULT '',
                thumbnail_url TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS playlist_tracks (
                playlist_id TEXT NOT NULL,
                video_id TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT NOT NULL,
                duration_secs INTEGER NOT NULL,
                duration_text TEXT NOT NULL DEFAULT '',
                thumbnail_url TEXT NOT NULL DEFAULT '',
                position INTEGER NOT NULL,
                PRIMARY KEY (playlist_id, position)
            );
            "#,
        )?;

        Ok(())
    }

    fn now_ts() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    pub fn add_liked_track(&self, track: &Track) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO liked_tracks (video_id, title, artist, album, duration_secs, thumbnail_url, added_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                track.video_id,
                track.title,
                track.artist,
                track.album,
                track.duration_secs,
                track.thumbnail_url,
                Self::now_ts()
            ],
        )?;
        Ok(())
    }

    pub fn remove_liked_track(&self, video_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM liked_tracks WHERE video_id = ?1",
            params![video_id],
        )?;
        Ok(())
    }

    pub fn is_track_liked(&self, video_id: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare("SELECT 1 FROM liked_tracks WHERE video_id = ?1")?;
        let exists = stmt.exists(params![video_id])?;
        Ok(exists)
    }

    pub fn get_liked_tracks(&self) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT video_id, title, artist, album, duration_secs, thumbnail_url FROM liked_tracks ORDER BY added_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let mut track = Track::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, String>(5)?,
            );
            track.is_liked = true;
            Ok(track)
        })?;

        let mut tracks = Vec::new();
        for r in rows {
            tracks.push(r?);
        }
        Ok(tracks)
    }

    pub fn add_history(&self, track: &Track) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO play_history (video_id, title, artist, album, duration_secs, thumbnail_url, played_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                track.video_id,
                track.title,
                track.artist,
                track.album,
                track.duration_secs,
                track.thumbnail_url,
                Self::now_ts()
            ],
        )?;
        Ok(())
    }

    pub fn get_history(&self, limit: usize) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT video_id, title, artist, album, duration_secs, thumbnail_url FROM play_history ORDER BY played_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let track = Track::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, String>(5)?,
            );
            Ok(track)
        })?;

        let mut tracks = Vec::new();
        for r in rows {
            tracks.push(r?);
        }
        Ok(tracks)
    }

    pub fn save_stream_url(&self, video_id: &str, stream_url: &str, ttl_secs: u64) -> Result<()> {
        let expires_at = Self::now_ts() + ttl_secs;
        self.conn.execute(
            "INSERT OR REPLACE INTO stream_cache (video_id, stream_url, expires_at) VALUES (?1, ?2, ?3)",
            params![video_id, stream_url, expires_at],
        )?;
        Ok(())
    }

    pub fn get_stream_url(&self, video_id: &str) -> Result<Option<String>> {
        let now = Self::now_ts();
        let mut stmt = self.conn.prepare(
            "SELECT stream_url FROM stream_cache WHERE video_id = ?1 AND expires_at > ?2",
        )?;
        let mut rows = stmt.query(params![video_id, now])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn save_liked_tracks(&self, tracks: &[Track]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM liked_tracks", [])?;
        let now = Self::now_ts();
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO liked_tracks (video_id, title, artist, album, duration_secs, thumbnail_url, added_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
            )?;
            for (idx, track) in tracks.iter().enumerate() {
                let ts = now.saturating_sub(idx as u64);
                stmt.execute(params![
                    track.video_id,
                    track.title,
                    track.artist,
                    track.album,
                    track.duration_secs,
                    track.thumbnail_url,
                    ts
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn save_playlists(&self, playlists: &[PlaylistSummary]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM playlists", [])?;
        let now = Self::now_ts();
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO playlists (id, title, subtitle, count_text, thumbnail_url, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
            )?;
            for pl in playlists {
                stmt.execute(params![
                    pl.id,
                    pl.title,
                    pl.subtitle,
                    pl.count_text,
                    pl.thumbnail_url,
                    now
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, subtitle, count_text, thumbnail_url FROM playlists ORDER BY rowid ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PlaylistSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                subtitle: row.get(2)?,
                count_text: row.get(3)?,
                thumbnail_url: row.get(4)?,
            })
        })?;

        let mut playlists = Vec::new();
        for r in rows {
            playlists.push(r?);
        }
        Ok(playlists)
    }

    pub fn save_playlist_tracks(&self, playlist_id: &str, tracks: &[Track]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
            params![playlist_id],
        )?;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO playlist_tracks (playlist_id, video_id, title, artist, album, duration_secs, duration_text, thumbnail_url, position)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                "#,
            )?;
            for (idx, track) in tracks.iter().enumerate() {
                stmt.execute(params![
                    playlist_id,
                    track.video_id,
                    track.title,
                    track.artist,
                    track.album,
                    track.duration_secs,
                    track.duration_text,
                    track.thumbnail_url,
                    idx as i64
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT video_id, title, artist, album, duration_secs, duration_text, thumbnail_url
            FROM playlist_tracks
            WHERE playlist_id = ?1
            ORDER BY position ASC
            "#,
        )?;
        let rows = stmt.query_map(params![playlist_id], |row| {
            let mut track = Track::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, String>(6)?,
            );
            track.duration_text = row.get::<_, String>(5)?;
            Ok(track)
        })?;

        let mut tracks = Vec::new();
        for r in rows {
            tracks.push(r?);
        }
        Ok(tracks)
    }

    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO app_metadata (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_metadata(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM app_metadata WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn set_last_sync_ts(&self, ts: u64) -> Result<()> {
        self.set_metadata("last_account_sync_ts", &ts.to_string())
    }

    pub fn get_last_sync_ts(&self) -> Result<Option<u64>> {
        if let Some(val) = self.get_metadata("last_account_sync_ts")? {
            Ok(val.parse::<u64>().ok())
        } else {
            Ok(None)
        }
    }

    pub fn save_user_profile(&self, prof: &UserProfile) -> Result<()> {
        self.set_metadata("user_profile_name", &prof.name)?;
        self.set_metadata("user_profile_handle", &prof.handle)?;
        self.set_metadata("user_profile_avatar_url", &prof.avatar_url)?;
        Ok(())
    }

    pub fn get_user_profile(&self) -> Result<Option<UserProfile>> {
        let name = self.get_metadata("user_profile_name")?;
        let handle = self.get_metadata("user_profile_handle")?;
        let avatar_url = self.get_metadata("user_profile_avatar_url")?;

        match (name, handle, avatar_url) {
            (Some(name), Some(handle), Some(avatar_url)) if !name.is_empty() => {
                Ok(Some(UserProfile {
                    name,
                    handle,
                    avatar_url,
                }))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_liked_tracks_crud() {
        let db = Database::open_in_memory().unwrap();
        let track = Track::new("id1", "Title 1", "Artist 1", "Album 1", 200, "thumb1");

        assert!(!db.is_track_liked("id1").unwrap());
        db.add_liked_track(&track).unwrap();
        assert!(db.is_track_liked("id1").unwrap());

        let liked = db.get_liked_tracks().unwrap();
        assert_eq!(liked.len(), 1);
        assert_eq!(liked[0].title, "Title 1");

        db.remove_liked_track("id1").unwrap();
        assert!(!db.is_track_liked("id1").unwrap());
    }

    #[test]
    fn test_save_liked_tracks_batch() {
        let db = Database::open_in_memory().unwrap();
        let tracks = vec![
            Track::new("id1", "Track 1", "Artist 1", "Album 1", 100, "thumb1"),
            Track::new("id2", "Track 2", "Artist 2", "Album 2", 200, "thumb2"),
        ];

        db.save_liked_tracks(&tracks).unwrap();
        let liked = db.get_liked_tracks().unwrap();
        assert_eq!(liked.len(), 2);
        assert_eq!(liked[0].title, "Track 1");
        assert_eq!(liked[1].title, "Track 2");
    }

    #[test]
    fn test_playlists_crud() {
        let db = Database::open_in_memory().unwrap();
        let playlists = vec![
            PlaylistSummary {
                id: "pl1".to_string(),
                title: "Rock Classics".to_string(),
                subtitle: "Playlist".to_string(),
                count_text: "50 músicas".to_string(),
                thumbnail_url: "https://example.com/thumb.jpg".to_string(),
            },
            PlaylistSummary {
                id: "pl2".to_string(),
                title: "Metal Hits".to_string(),
                subtitle: "Playlist".to_string(),
                count_text: "30 músicas".to_string(),
                thumbnail_url: "https://example.com/thumb2.jpg".to_string(),
            },
        ];

        db.save_playlists(&playlists).unwrap();
        let loaded = db.get_playlists().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].title, "Rock Classics");
        assert_eq!(loaded[1].title, "Metal Hits");
        assert_eq!(loaded[1].count_text, "30 músicas");
    }

    #[test]
    fn test_playlist_tracks_crud() {
        let db = Database::open_in_memory().unwrap();
        let tracks = vec![
            Track::new("vid1", "Song A", "Band X", "Album 1", 180, "thumbA"),
            Track::new("vid2", "Song B", "Band Y", "Album 2", 240, "thumbB"),
        ];

        db.save_playlist_tracks("pl_heavy", &tracks).unwrap();
        let loaded = db.get_playlist_tracks("pl_heavy").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].video_id, "vid1");
        assert_eq!(loaded[0].duration_text, "03:00");
        assert_eq!(loaded[1].video_id, "vid2");
        assert_eq!(loaded[1].duration_text, "04:00");

        let empty = db.get_playlist_tracks("non_existent").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_metadata_and_user_profile() {
        let db = Database::open_in_memory().unwrap();

        assert!(db.get_user_profile().unwrap().is_none());
        assert!(db.get_last_sync_ts().unwrap().is_none());

        let prof = UserProfile {
            name: "Jean".to_string(),
            handle: "@jean".to_string(),
            avatar_url: "https://lh3.googleusercontent.com/avatar".to_string(),
        };

        db.save_user_profile(&prof).unwrap();
        db.set_last_sync_ts(1700000000).unwrap();

        let loaded_prof = db.get_user_profile().unwrap().unwrap();
        assert_eq!(loaded_prof.name, "Jean");
        assert_eq!(loaded_prof.handle, "@jean");
        assert_eq!(loaded_prof.avatar_url, "https://lh3.googleusercontent.com/avatar");

        assert_eq!(db.get_last_sync_ts().unwrap(), Some(1700000000));
    }

    #[test]
    fn test_stream_cache() {
        let db = Database::open_in_memory().unwrap();
        db.save_stream_url("id2", "https://stream.url", 3600).unwrap();
        let cached = db.get_stream_url("id2").unwrap();
        assert_eq!(cached, Some("https://stream.url".to_string()));
    }
}
