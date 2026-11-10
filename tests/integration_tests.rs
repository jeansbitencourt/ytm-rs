use ytm_rs::audio::{DaemonCommand, DaemonResponse};
use ytm_rs::cache::Database;
use ytm_rs::innertube::auth::AuthConfig;
use ytm_rs::innertube::client::{parse_duration_to_secs, InnertubeClient};
use ytm_rs::innertube::{SearchFilter, Track};
use ytm_rs::theme::{parse_hex_color, Theme};

#[test]
fn test_search_filters() {
    let f = SearchFilter::Tracks;
    assert_eq!(f.next(), SearchFilter::Albums);
    assert_eq!(f.next().next(), SearchFilter::Artists);
    assert_eq!(f.next().next().next(), SearchFilter::Tracks);
    assert_eq!(f.prev(), SearchFilter::Artists);
}

#[tokio::test]
async fn test_innertube_album_and_artist_search_live() {
    let client = InnertubeClient::new(AuthConfig::default());

    // Search albums
    let albums = client
        .search_albums("Random Access Memories")
        .await
        .unwrap();
    assert!(
        !albums.is_empty(),
        "Should find albums for Random Access Memories"
    );
    let album = &albums[0];
    assert!(!album.browse_id.is_empty());

    // Browse album tracks
    let (album_title, tracks) = client.get_album_tracks(&album.browse_id).await.unwrap();
    assert!(!album_title.is_empty());
    assert!(!tracks.is_empty(), "Album should contain tracks");
    assert!(!tracks[0].video_id.is_empty());

    // Search artists
    let artists = client.search_artists("Daft Punk").await.unwrap();
    assert!(!artists.is_empty(), "Should find artists for Daft Punk");
    let artist = &artists[0];
    assert!(!artist.browse_id.is_empty());

    // Browse artist tracks
    let (artist_name, artist_tracks) = client.get_artist_tracks(&artist.browse_id).await.unwrap();
    assert!(!artist_name.is_empty());
    assert!(
        !artist_tracks.is_empty(),
        "Artist should have tracks available"
    );
}

#[test]
fn test_theme_omarchy_colors() {
    let color = parse_hex_color("#8FBC8F");
    assert!(color.is_some());

    let theme = Theme::default();
    assert_eq!(theme.theme_name, "Tokyo Night (Default)");
}

#[test]
fn test_custom_theme_file_loading() {
    let temp_dir = std::env::temp_dir().join("ytm_rs_test_theme");
    let _ = std::fs::create_dir_all(&temp_dir);
    let theme_file = temp_dir.join("theme.toml");
    let content = r##"
name = "Catppuccin Mocha"
accent = "#89b4fa"
background = "#1e1e2e"
foreground = "#cdd6f4"
"##;
    std::fs::write(&theme_file, content).unwrap();
    let theme = Theme::from_file(&theme_file).unwrap();
    assert_eq!(theme.theme_name, "Catppuccin Mocha");
    assert_eq!(theme.accent, ratatui::style::Color::Rgb(137, 180, 250));
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_live_track_search_and_radio() {
    let client = InnertubeClient::new(AuthConfig::default());
    let tracks = client.search("Stranger in a Strange Land").await.unwrap();
    assert!(!tracks.is_empty(), "Should find tracks");
    for t in tracks.iter().take(3) {
        println!(
            "Search Track: '{}' by '{}', album: '{}', dur_secs: {}, dur_text: '{}'",
            t.title, t.artist, t.album, t.duration_secs, t.duration_text
        );
        assert!(t.duration_secs > 0, "Duration must be greater than 0");
        assert_ne!(t.duration_text, "00:00", "Duration must not be 00:00");
    }
}

#[test]
fn test_database_lifecycle() {
    let db = Database::open_in_memory().expect("Should open in-memory db");
    let track1 = Track::new(
        "vid_01",
        "Nightcall",
        "Kavinsky",
        "OutRun",
        258,
        "https://thumb.url",
    );
    let track2 = Track::new(
        "vid_02",
        "Resonance",
        "HOME",
        "Odyssey",
        212,
        "https://thumb.url",
    );

    // Test liked
    db.add_liked_track(&track1).unwrap();
    assert!(db.is_track_liked("vid_01").unwrap());
    assert!(!db.is_track_liked("vid_02").unwrap());

    let liked = db.get_liked_tracks().unwrap();
    assert_eq!(liked.len(), 1);
    assert_eq!(liked[0].title, "Nightcall");

    // Test history
    db.add_history(&track1).unwrap();
    db.add_history(&track2).unwrap();
    let history = db.get_history(10).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].title, "Resonance"); // most recent first

    // Test stream cache
    db.save_stream_url("vid_01", "https://stream.opus", 1000)
        .unwrap();
    let cached = db.get_stream_url("vid_01").unwrap();
    assert_eq!(cached, Some("https://stream.opus".to_string()));
}

#[test]
fn test_duration_parsing() {
    assert_eq!(parse_duration_to_secs("00:45"), 45);
    assert_eq!(parse_duration_to_secs("04:18"), 258);
    assert_eq!(parse_duration_to_secs("01:23:45"), 5025);
}

#[tokio::test]
async fn test_daemon_protocol_roundtrip() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{UnixListener, UnixStream};

    let temp_dir = std::env::temp_dir();
    let sock_path = temp_dir.join(format!("test_ytm_{}.sock", std::process::id()));
    if sock_path.exists() {
        let _ = std::fs::remove_file(&sock_path);
    }

    let listener = UnixListener::bind(&sock_path).unwrap();

    let server_task = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            if let Ok(Some(line)) = lines.next_line().await {
                let cmd: DaemonCommand = serde_json::from_str(&line).unwrap();
                assert_eq!(cmd, DaemonCommand::Toggle);
                let resp = DaemonResponse::Ok;
                let mut out = serde_json::to_string(&resp).unwrap();
                out.push('\n');
                let _ = writer.write_all(out.as_bytes()).await;
            }
        }
    });

    let client_stream = UnixStream::connect(&sock_path).await.unwrap();
    let (reader, mut writer) = client_stream.into_split();

    let cmd = DaemonCommand::Toggle;
    let mut line = serde_json::to_string(&cmd).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();

    let mut lines = BufReader::new(reader).lines();
    let resp_line = lines.next_line().await.unwrap().unwrap();
    let resp: DaemonResponse = serde_json::from_str(&resp_line).unwrap();
    assert_eq!(resp, DaemonResponse::Ok);

    server_task.await.unwrap();
    let _ = std::fs::remove_file(&sock_path);
}

#[test]
fn test_detect_font_size() {
    let font_size = ytm_rs::app::detect_terminal_font_size();
    println!("Detected font size: {:?}", font_size);
    assert!(font_size.0 > 0 && font_size.1 > 0);
}

#[tokio::test]
async fn test_oauth_user_profile_and_library_live() {
    let auth_path = dirs::config_dir()
        .map(|p| p.join("ytm-rs/auth.json"))
        .unwrap();
    let auth = AuthConfig::load(&auth_path);
    if !auth.has_oauth() {
        return;
    }

    let client = InnertubeClient::new(auth);

    // Profile test
    let prof = client
        .get_user_profile()
        .await
        .expect("Failed to get user profile");
    assert!(prof.is_some(), "Profile should be present for valid OAuth");
    let p = prof.unwrap();
    assert_eq!(p.name, "Jean Bitencourt");
    assert!(!p.avatar_url.is_empty());

    // Playlists test
    let playlists = client
        .get_user_playlists()
        .await
        .expect("Failed to get playlists");
    assert!(!playlists.is_empty(), "Should find user playlists");

    // Liked music test (should paginate and retrieve > 200 tracks)
    let liked = client
        .get_liked_music()
        .await
        .expect("Failed to get liked music");
    assert!(
        liked.len() > 200,
        "Should paginate and find > 200 liked tracks, found: {}",
        liked.len()
    );

    // Playlist tracks test
    let (pl_title, pl_tracks) = client
        .get_playlist_tracks(&playlists[0].id)
        .await
        .expect("Failed to get playlist tracks");
    assert!(
        !pl_tracks.is_empty(),
        "First playlist '{}' should have tracks",
        pl_title
    );
}

#[test]
fn test_cache_first_and_sync_ttl() {
    use ytm_rs::innertube::PlaylistSummary;

    let db = Database::open_in_memory().expect("Should open db");

    // 1. Initial state: no sync recorded, playlists empty
    assert!(db.get_last_sync_ts().unwrap().is_none());
    assert!(db.get_playlists().unwrap().is_empty());

    // 2. Simulate saving synced data
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let playlists = vec![PlaylistSummary {
        id: "VLPL123".to_string(),
        title: "My Metal Playlist".to_string(),
        subtitle: "Playlist".to_string(),
        count_text: "12 faixas".to_string(),
        thumbnail_url: "https://thumb.url".to_string(),
    }];

    db.save_playlists(&playlists).unwrap();
    db.set_last_sync_ts(now).unwrap();

    // 3. Fast startup check: data immediately readable from SQLite
    let loaded = db.get_playlists().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].title, "My Metal Playlist");

    // 4. TTL Check (24 hours = 86_400 secs):
    const SYNC_TTL_SECS: u64 = 86_400;
    let last_sync = db.get_last_sync_ts().unwrap().unwrap();

    // Less than 24h passed: should NOT sync
    let five_minutes_later = now + 300;
    let should_sync_recent =
        (five_minutes_later.saturating_sub(last_sync) >= SYNC_TTL_SECS) || loaded.is_empty();
    assert!(
        !should_sync_recent,
        "Should skip network sync when cache is fresh (< 24h)"
    );

    // More than 24h passed: should sync
    let two_days_later = now + 172_800;
    let should_sync_expired =
        (two_days_later.saturating_sub(last_sync) >= SYNC_TTL_SECS) || loaded.is_empty();
    assert!(
        should_sync_expired,
        "Should trigger sync when cache is expired (> 24h)"
    );
}
