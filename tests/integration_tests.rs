use ytm_rs::audio::{DaemonCommand, DaemonResponse};
use ytm_rs::cache::Database;
use ytm_rs::innertube::client::parse_duration_to_secs;
use ytm_rs::innertube::Track;
use ytm_rs::theme::{parse_hex_color, Theme};

#[test]
fn test_theme_omarchy_colors() {
    let color = parse_hex_color("#8FBC8F");
    assert!(color.is_some());

    let theme = Theme::default();
    assert_eq!(theme.theme_name, "Tokyo Night (Default)");
}

#[test]
fn test_database_lifecycle() {
    let db = Database::open_in_memory().expect("Should open in-memory db");
    let track1 = Track::new("vid_01", "Nightcall", "Kavinsky", "OutRun", 258, "https://thumb.url");
    let track2 = Track::new("vid_02", "Resonance", "HOME", "Odyssey", 212, "https://thumb.url");

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
    db.save_stream_url("vid_01", "https://stream.opus", 1000).unwrap();
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
