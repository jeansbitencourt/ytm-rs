use serde_json::Value;

use super::{AlbumSummary, ArtistSummary, PlaylistSummary, Track, UserProfile};

pub fn parse_duration_to_secs(text: &str) -> u32 {
    let parts: Vec<&str> = text.split(':').collect();
    match parts.len() {
        1 => parts[0].parse().unwrap_or(0),
        2 => {
            let m: u32 = parts[0].parse().unwrap_or(0);
            let s: u32 = parts[1].parse().unwrap_or(0);
            m * 60 + s
        }
        3 => {
            let h: u32 = parts[0].parse().unwrap_or(0);
            let m: u32 = parts[1].parse().unwrap_or(0);
            let s: u32 = parts[2].parse().unwrap_or(0);
            h * 3600 + m * 60 + s
        }
        _ => 0,
    }
}

pub fn is_view_count(s: &str) -> bool {
    let lower = s.trim().to_lowercase();
    lower.contains("visualiza")
        || lower.contains("views")
        || lower.ends_with("view")
        || lower.contains("reproduç")
        || lower.contains("reproduc")
        || lower.contains("ouvintes")
        || lower.contains("listeners")
        || lower.contains("assistir")
        || lower.contains("plays")
}

pub fn is_duration_str(s: &str) -> bool {
    let trimmed = s.trim();
    trimmed.contains(':') && trimmed.chars().all(|c| c.is_ascii_digit() || c == ':')
}

pub fn is_category_label(s: &str) -> bool {
    let lower = s.trim().to_lowercase();
    matches!(
        lower.as_str(),
        "música"
            | "musica"
            | "vídeo"
            | "video"
            | "canção"
            | "faixa"
            | "song"
            | "track"
            | "single"
            | "ep"
    )
}

fn has_official_video_endpoint(value: &Value, video_id: &str) -> bool {
    match value {
        Value::Object(object) => {
            let is_explicitly_typed_video = object.get("videoId").and_then(Value::as_str)
                == Some(video_id)
                && object.get("musicVideoType").and_then(Value::as_str)
                    == Some("MUSIC_VIDEO_TYPE_OMV");
            let is_matching_official_video = object
                .get("watchEndpoint")
                .and_then(Value::as_object)
                .is_some_and(|endpoint| {
                    endpoint.get("videoId").and_then(Value::as_str) == Some(video_id)
                        && endpoint
                            .get("watchEndpointMusicSupportedConfigs")
                            .and_then(|value| value.get("watchEndpointMusicConfig"))
                            .and_then(|value| value.get("musicVideoType"))
                            .and_then(Value::as_str)
                            == Some("MUSIC_VIDEO_TYPE_OMV")
                });
            is_explicitly_typed_video
                || is_matching_official_video
                || object
                    .values()
                    .any(|child| has_official_video_endpoint(child, video_id))
        }
        Value::Array(items) => items
            .iter()
            .any(|child| has_official_video_endpoint(child, video_id)),
        _ => false,
    }
}

fn explicit_music_video_id(renderer: &Value, video_id: &str) -> Option<String> {
    has_official_video_endpoint(renderer, video_id).then(|| video_id.to_string())
}

fn counterpart_video_id(wrapper: &Value, primary_video_id: &str) -> Option<String> {
    let primary_id = wrapper
        .pointer("/primaryRenderer/playlistPanelVideoRenderer/videoId")
        .and_then(Value::as_str)?;
    if primary_id != primary_video_id {
        return None;
    }

    wrapper
        .pointer("/counterpart/0/counterpartRenderer/playlistPanelVideoRenderer/videoId")
        .and_then(Value::as_str)
        .filter(|video_id| is_valid_video_id(video_id))
        .map(str::to_string)
        .or_else(|| {
            let primary = wrapper.pointer("/primaryRenderer/playlistPanelVideoRenderer")?;
            has_official_video_endpoint(primary, primary_video_id)
                .then(|| primary_video_id.to_string())
        })
}

fn find_counterpart_video_id(value: &Value, primary_video_id: &str) -> Option<String> {
    match value {
        Value::Object(object) => {
            if let Some(wrapper) = object.get("playlistPanelVideoWrapperRenderer") {
                if let Some(video_id) = counterpart_video_id(wrapper, primary_video_id) {
                    return Some(video_id);
                }
            }
            object
                .values()
                .find_map(|child| find_counterpart_video_id(child, primary_video_id))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_counterpart_video_id(child, primary_video_id)),
        _ => None,
    }
}

fn is_valid_video_id(video_id: &str) -> bool {
    video_id.len() == 11
        && video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Finds the exact audio-to-video counterpart returned by the watch playlist.
pub fn parse_music_video_counterpart(response: &Value, audio_video_id: &str) -> Option<String> {
    find_counterpart_video_id(response, audio_video_id).or_else(|| {
        has_official_video_endpoint(response, audio_video_id).then(|| audio_video_id.to_string())
    })
}

pub fn parse_search_results(val: &Value) -> Vec<Track> {
    let mut tracks = Vec::new();
    let items = val
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .or_else(|| val.pointer("/contents/sectionListRenderer/contents"));

    if let Some(Value::Array(sections)) = items {
        for sec in sections {
            let contents = sec
                .pointer("/musicShelfRenderer/contents")
                .or_else(|| sec.pointer("/musicCardShelfRenderer/contents"))
                .or_else(|| sec.pointer("/itemSectionRenderer/contents"));

            if let Some(Value::Array(items)) = contents {
                for item in items {
                    if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                        if let Some(track) = parse_music_responsive_item(renderer) {
                            tracks.push(track);
                        }
                    }
                }
            }
        }
    }
    tracks
}

pub fn parse_music_responsive_item(renderer: &Value) -> Option<Track> {
    let video_id = renderer
        .pointer("/playlistItemData/videoId")
        .or_else(|| renderer.pointer("/doubleTapCommand/watchEndpoint/videoId"))
        .or_else(|| renderer.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId"))
        .or_else(|| renderer.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/watchEndpoint/videoId"))
        .and_then(|v| v.as_str())?
        .to_string();

    let flex_columns = renderer.get("flexColumns")?.as_array()?;
    if flex_columns.is_empty() {
        return None;
    }

    let title_runs = flex_columns[0]
        .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?
        .as_array()?;
    let title = title_runs
        .iter()
        .map(|r| r.get("text").and_then(|t| t.as_str()).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("");

    let mut artist = "Artista Desconhecido".to_string();
    let mut album = "".to_string();
    let mut duration_secs = 0;

    for col in &flex_columns[1..] {
        if let Some(runs) = col
            .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")
            .and_then(|r| r.as_array())
        {
            for r in runs {
                let text = r.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
                if text.is_empty() || text == "•" {
                    continue;
                }
                let page_type = r.pointer("/navigationEndpoint/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
                    .and_then(|p| p.as_str());

                if page_type == Some("MUSIC_PAGE_TYPE_ALBUM") {
                    album = text.to_string();
                    continue;
                }
                if page_type == Some("MUSIC_PAGE_TYPE_ARTIST") {
                    artist = text.to_string();
                    continue;
                }
                if is_duration_str(text) {
                    let d = parse_duration_to_secs(text);
                    if d > 0 {
                        duration_secs = d;
                    }
                    continue;
                }
                if is_view_count(text) || is_category_label(text) {
                    continue;
                }
                if artist == "Artista Desconhecido" || artist.is_empty() {
                    artist = text.to_string();
                } else if album.is_empty() && !text.chars().all(|c| c.is_ascii_digit()) {
                    album = text.to_string();
                }
            }
        }
    }

    if duration_secs == 0 {
        if let Some(fixed_cols) = renderer.get("fixedColumns").and_then(|f| f.as_array()) {
            for col in fixed_cols {
                if let Some(dur_text) = col
                    .pointer("/musicResponsiveListItemFixedColumnRenderer/text/runs/0/text")
                    .or_else(|| {
                        col.pointer("/musicResponsiveListItemFixedColumnRenderer/text/simpleText")
                    })
                    .and_then(|t| t.as_str())
                {
                    let d = parse_duration_to_secs(dur_text);
                    if d > 0 {
                        duration_secs = d;
                        break;
                    }
                }
            }
        }
    }

    let thumbnail_url = renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    let music_video_id = explicit_music_video_id(renderer, &video_id);
    Some(
        Track::new(video_id, title, artist, album, duration_secs, thumbnail_url)
            .with_music_video_id(music_video_id),
    )
}

pub fn parse_radio_results(val: &Value) -> Vec<Track> {
    let mut tracks = Vec::new();
    let items = val.pointer("/continuationContents/playlistPanelContinuation/contents")
        .or_else(|| val.pointer("/contents/singleColumnMusicWatchNextResultsRenderer/tabbedRenderer/watchNextTabbedResultsRenderer/tabs/0/tabRenderer/content/musicQueueRenderer/content/playlistPanelRenderer/contents"))
        .or_else(|| val.pointer("/currentVideoEndpoint/watchEndpoint/playlistPanelRenderer/contents"));

    if let Some(Value::Array(list)) = items {
        for item in list {
            let wrapper = item.get("playlistPanelVideoWrapperRenderer");
            let renderer = item.get("playlistPanelVideoRenderer").or_else(|| {
                wrapper
                    .and_then(|value| value.pointer("/primaryRenderer/playlistPanelVideoRenderer"))
            });
            if let Some(renderer) = renderer {
                if let Some(mut track) = parse_playlist_panel_video(renderer) {
                    if let Some(wrapper) = wrapper {
                        if let Some(video_id) = counterpart_video_id(wrapper, &track.video_id) {
                            track.music_video_id = Some(video_id);
                        }
                    }
                    tracks.push(track);
                }
            }
        }
    }
    tracks
}

pub fn parse_playlist_panel_video(renderer: &Value) -> Option<Track> {
    let video_id = renderer.get("videoId")?.as_str()?.to_string();
    let title = renderer
        .pointer("/title/runs/0/text")?
        .as_str()?
        .to_string();

    let mut artist = "Artista Desconhecido".to_string();
    let mut album = "".to_string();
    let mut duration_secs = 0;

    if let Some(runs) = renderer
        .pointer("/longBylineText/runs")
        .or_else(|| renderer.pointer("/shortBylineText/runs"))
        .and_then(|r| r.as_array())
    {
        for r in runs {
            let text = r.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
            if text.is_empty() || text == "•" {
                continue;
            }
            let page_type = r.pointer("/navigationEndpoint/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
                .and_then(|p| p.as_str());

            if page_type == Some("MUSIC_PAGE_TYPE_ALBUM") {
                album = text.to_string();
                continue;
            }
            if page_type == Some("MUSIC_PAGE_TYPE_ARTIST") {
                artist = text.to_string();
                continue;
            }
            if is_duration_str(text) {
                if duration_secs == 0 {
                    duration_secs = parse_duration_to_secs(text);
                }
                continue;
            }
            if is_view_count(text) || is_category_label(text) {
                continue;
            }
            if artist == "Artista Desconhecido" || artist.is_empty() {
                artist = text.to_string();
            } else if album.is_empty() && !text.chars().all(|c| c.is_ascii_digit()) {
                album = text.to_string();
            }
        }
    }

    if duration_secs == 0 {
        if let Some(dur_text) = renderer
            .pointer("/lengthText/runs/0/text")
            .or_else(|| renderer.pointer("/lengthText/simpleText"))
            .and_then(|t| t.as_str())
        {
            duration_secs = parse_duration_to_secs(dur_text);
        }
    }

    let thumbnail_url = renderer
        .pointer("/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    let music_video_id = explicit_music_video_id(renderer, &video_id);
    Some(
        Track::new(video_id, title, artist, album, duration_secs, thumbnail_url)
            .with_music_video_id(music_video_id),
    )
}

pub fn parse_playlist_results(val: &Value) -> Vec<Track> {
    let mut tracks = Vec::new();

    // 1. TVHTML5 Liked songs or browse tab
    if let Some(tabs) = val
        .pointer("/contents/tvBrowseRenderer/content/tvSecondaryNavRenderer/sections/0/tvSecondaryNavSectionRenderer/tabs")
        .and_then(|v| v.as_array())
    {
        for tab in tabs {
            let tr = tab.get("tabRenderer");
            let is_selected = tr.and_then(|t| t.get("selected")).and_then(|s| s.as_bool()).unwrap_or(false);
            let title = tr.and_then(|t| t.get("title")).and_then(|s| s.as_str()).unwrap_or("");
            if is_selected || title.contains("Liked") || title.contains("Curtidas") {
                if let Some(items) = tr.and_then(|t| t.pointer("/content/tvSurfaceContentRenderer/content/gridRenderer/items")).and_then(|v| v.as_array()) {
                    for it in items {
                        if let Some(tile) = it.get("tileRenderer") {
                            if let Some(track) = parse_tv_tile_track(tile) {
                                tracks.push(track);
                            }
                        }
                    }
                    if !tracks.is_empty() {
                        return tracks;
                    }
                }
            }
        }
    }

    // 2. TVHTML5 Playlist detail list (right column)
    if let Some(contents) = val
        .pointer("/contents/tvBrowseRenderer/content/tvSurfaceContentRenderer/content/twoColumnRenderer/rightColumn/playlistVideoListRenderer/contents")
        .and_then(|v| v.as_array())
    {
        for it in contents {
            if let Some(tile) = it.get("tileRenderer") {
                if let Some(track) = parse_tv_tile_track(tile) {
                    tracks.push(track);
                }
            }
        }
        if !tracks.is_empty() {
            return tracks;
        }
    }

    // 3. Web Remix formats
    let items = val
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicPlaylistShelfRenderer/contents")
        .or_else(|| val.pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer/contents/0/musicPlaylistShelfRenderer/contents"))
        .or_else(|| val.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicShelfRenderer/contents"));

    if let Some(Value::Array(list)) = items {
        for item in list {
            if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                if let Some(track) = parse_music_responsive_item(renderer) {
                    tracks.push(track);
                }
            }
        }
    }
    tracks
}

/// Extracts the next continuation token from an initial browse response
pub fn extract_browse_continuation_token(val: &Value) -> Option<String> {
    // 1. TVHTML5 Liked songs tab gridRenderer continuations
    if let Some(tabs) = val
        .pointer("/contents/tvBrowseRenderer/content/tvSecondaryNavRenderer/sections/0/tvSecondaryNavSectionRenderer/tabs")
        .and_then(|v| v.as_array())
    {
        for tab in tabs {
            let tr = tab.get("tabRenderer");
            let is_selected = tr.and_then(|t| t.get("selected")).and_then(|s| s.as_bool()).unwrap_or(false);
            let title = tr.and_then(|t| t.get("title")).and_then(|s| s.as_str()).unwrap_or("");
            if is_selected || title.contains("Liked") || title.contains("Curtidas") {
                if let Some(token) = tr.and_then(|t| t.pointer("/content/tvSurfaceContentRenderer/content/gridRenderer/continuations/0/nextContinuationData/continuation")).and_then(|v| v.as_str()) {
                    return Some(token.to_string());
                }
            }
        }
    }

    // 2. TVHTML5 Playlist detail list (right column)
    if let Some(token) = val
        .pointer("/contents/tvBrowseRenderer/content/tvSurfaceContentRenderer/content/twoColumnRenderer/rightColumn/playlistVideoListRenderer/continuations/0/nextContinuationData/continuation")
        .and_then(|v| v.as_str())
    {
        return Some(token.to_string());
    }

    // 3. Web Remix formats
    let token = val
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicPlaylistShelfRenderer/continuations/0/nextContinuationData/continuation")
        .or_else(|| val.pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer/contents/0/musicPlaylistShelfRenderer/continuations/0/nextContinuationData/continuation"))
        .or_else(|| val.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicShelfRenderer/continuations/0/nextContinuationData/continuation"))
        .or_else(|| val.pointer("/contents/sectionListRenderer/continuations/0/nextContinuationData/continuation"))
        .and_then(|v| v.as_str());

    token.map(|s| s.to_string())
}

/// Parses tracks and the next continuation token from a continuation response
pub fn parse_continuation_response(val: &Value) -> (Vec<Track>, Option<String>) {
    let mut tracks = Vec::new();
    let mut next_token = None;

    // 1. TVHTML5 gridContinuation (Liked music)
    if let Some(gc) = val.pointer("/continuationContents/gridContinuation") {
        if let Some(items) = gc.get("items").and_then(|v| v.as_array()) {
            for it in items {
                if let Some(tile) = it.get("tileRenderer") {
                    if let Some(track) = parse_tv_tile_track(tile) {
                        tracks.push(track);
                    }
                }
            }
        }
        if let Some(tok) = gc
            .pointer("/continuations/0/nextContinuationData/continuation")
            .and_then(|v| v.as_str())
        {
            next_token = Some(tok.to_string());
        }
        return (tracks, next_token);
    }

    // 2. TVHTML5 playlistVideoListContinuation (Playlists)
    if let Some(pvlc) = val.pointer("/continuationContents/playlistVideoListContinuation") {
        if let Some(contents) = pvlc.get("contents").and_then(|v| v.as_array()) {
            for it in contents {
                if let Some(tile) = it.get("tileRenderer") {
                    if let Some(track) = parse_tv_tile_track(tile) {
                        tracks.push(track);
                    }
                }
            }
        }
        if let Some(tok) = pvlc
            .pointer("/continuations/0/nextContinuationData/continuation")
            .and_then(|v| v.as_str())
        {
            next_token = Some(tok.to_string());
        }
        return (tracks, next_token);
    }

    // 3. Web remix continuationContents (musicPlaylistShelfContinuation or musicShelfContinuation)
    let shelf_cont = val
        .pointer("/continuationContents/musicPlaylistShelfContinuation")
        .or_else(|| val.pointer("/continuationContents/musicShelfContinuation"))
        .or_else(|| val.pointer("/continuationContents/sectionListContinuation"));

    if let Some(sc) = shelf_cont {
        if let Some(contents) = sc.get("contents").and_then(|v| v.as_array()) {
            for item in contents {
                if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                    if let Some(track) = parse_music_responsive_item(renderer) {
                        tracks.push(track);
                    }
                }
            }
        }
        if let Some(tok) = sc
            .pointer("/continuations/0/nextContinuationData/continuation")
            .and_then(|v| v.as_str())
        {
            next_token = Some(tok.to_string());
        }
        return (tracks, next_token);
    }

    // 4. Web remix appendContinuationItemsAction
    if let Some(items) = val
        .pointer("/onResponseReceivedActions/0/appendContinuationItemsAction/continuationItems")
        .and_then(|v| v.as_array())
    {
        for item in items {
            if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                if let Some(track) = parse_music_responsive_item(renderer) {
                    tracks.push(track);
                }
            } else if let Some(tok) = item
                .pointer("/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
                .and_then(|v| v.as_str())
            {
                next_token = Some(tok.to_string());
            }
        }
    }

    (tracks, next_token)
}

pub fn parse_album_search_results(val: &Value) -> Vec<AlbumSummary> {
    let mut albums = Vec::new();
    let items = val
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .or_else(|| val.pointer("/contents/sectionListRenderer/contents"));

    if let Some(Value::Array(sections)) = items {
        for sec in sections {
            let contents = sec
                .pointer("/musicShelfRenderer/contents")
                .or_else(|| sec.pointer("/musicCardShelfRenderer/contents"))
                .or_else(|| sec.pointer("/itemSectionRenderer/contents"));

            if let Some(Value::Array(items)) = contents {
                for item in items {
                    if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                        if let Some(album) = parse_album_item(renderer) {
                            albums.push(album);
                        }
                    }
                }
            }
        }
    }
    albums
}

pub fn parse_album_item(renderer: &Value) -> Option<AlbumSummary> {
    let browse_id = renderer
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .or_else(|| renderer.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(|v| v.as_str())?
        .to_string();

    let playlist_id = renderer
        .pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchPlaylistEndpoint/playlistId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let flex_columns = renderer.get("flexColumns")?.as_array()?;
    if flex_columns.is_empty() {
        return None;
    }

    let title_runs = flex_columns[0]
        .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?
        .as_array()?;
    let title = title_runs
        .iter()
        .map(|r| r.get("text").and_then(|t| t.as_str()).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("");

    let mut artist = "Artista Desconhecido".to_string();
    let mut year = "".to_string();

    if flex_columns.len() > 1 {
        if let Some(subtitle_runs) = flex_columns[1]
            .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")
            .and_then(|r| r.as_array())
        {
            let texts: Vec<&str> = subtitle_runs
                .iter()
                .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
                .filter(|t| *t != " • ")
                .collect();
            for t in &texts {
                let s = t.trim();
                if s.chars().all(|c| c.is_ascii_digit()) && s.len() == 4 {
                    year = s.to_string();
                } else if s != "Álbum"
                    && s != "Album"
                    && s != "Single"
                    && s != "EP"
                    && (artist == "Artista Desconhecido" || artist.is_empty())
                {
                    artist = s.to_string();
                }
            }
        }
    }

    let thumbnail_url = renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(AlbumSummary {
        browse_id,
        playlist_id,
        title,
        artist,
        year,
        thumbnail_url,
    })
}

pub fn parse_artist_search_results(val: &Value) -> Vec<ArtistSummary> {
    let mut artists = Vec::new();
    let items = val
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .or_else(|| val.pointer("/contents/sectionListRenderer/contents"));

    if let Some(Value::Array(sections)) = items {
        for sec in sections {
            let contents = sec
                .pointer("/musicShelfRenderer/contents")
                .or_else(|| sec.pointer("/musicCardShelfRenderer/contents"))
                .or_else(|| sec.pointer("/itemSectionRenderer/contents"));

            if let Some(Value::Array(items)) = contents {
                for item in items {
                    if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                        if let Some(artist) = parse_artist_item(renderer) {
                            artists.push(artist);
                        }
                    }
                }
            }
        }
    }
    artists
}

pub fn parse_artist_item(renderer: &Value) -> Option<ArtistSummary> {
    let browse_id = renderer
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .or_else(|| renderer.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(|v| v.as_str())?
        .to_string();

    let flex_columns = renderer.get("flexColumns")?.as_array()?;
    if flex_columns.is_empty() {
        return None;
    }

    let name_runs = flex_columns[0]
        .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?
        .as_array()?;
    let name = name_runs
        .iter()
        .map(|r| r.get("text").and_then(|t| t.as_str()).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("");

    let mut subscribers_text = "".to_string();
    if flex_columns.len() > 1 {
        if let Some(subtitle_runs) = flex_columns[1]
            .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")
            .and_then(|r| r.as_array())
        {
            let texts: Vec<&str> = subtitle_runs
                .iter()
                .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
                .filter(|t| *t != " • ")
                .collect();
            if texts.len() > 1 {
                subscribers_text = texts[1].to_string();
            } else if !texts.is_empty() {
                subscribers_text = texts[0].to_string();
            }
        }
    }

    let thumbnail_url = renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(ArtistSummary {
        browse_id,
        name,
        subscribers_text,
        thumbnail_url,
    })
}

pub fn parse_album_browse_tracks(val: &Value) -> (String, Vec<Track>) {
    let header = val.pointer("/header/musicResponsiveHeaderRenderer")
        .or_else(|| val.pointer("/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicResponsiveHeaderRenderer"));

    let album_title = header
        .and_then(|h| h.pointer("/title/runs/0/text"))
        .and_then(|t| t.as_str())
        .unwrap_or("Álbum")
        .to_string();

    let album_artist = header
        .and_then(|h| {
            h.pointer("/straplineTextOne/runs/0/text")
                .or_else(|| h.pointer("/subtitle/runs/0/text"))
        })
        .and_then(|t| t.as_str())
        .unwrap_or("Artista Desconhecido")
        .to_string();

    let header_thumb = header
        .and_then(|h| h.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails"))
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    let mut tracks = Vec::new();

    let mut raw_items = Vec::new();
    if let Some(items) = val.pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer/contents/0/musicShelfRenderer/contents").and_then(|v| v.as_array()) {
        raw_items.extend(items.clone());
    } else if let Some(items) = val.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicShelfRenderer/contents").and_then(|v| v.as_array()) {
        raw_items.extend(items.clone());
    } else if let Some(sections) = val.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents").and_then(|v| v.as_array()) {
        for s in sections {
            if let Some(contents) = s.pointer("/musicShelfRenderer/contents").and_then(|v| v.as_array()) {
                raw_items.extend(contents.clone());
            }
        }
    }

    for item in raw_items {
        if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
            if let Some(mut track) = parse_music_responsive_item(renderer) {
                if track.album.is_empty() {
                    track.album = album_title.clone();
                }
                if track.artist == "Artista Desconhecido" {
                    track.artist = album_artist.clone();
                }
                if track.thumbnail_url.is_empty() && !header_thumb.is_empty() {
                    track.thumbnail_url = header_thumb.clone();
                }
                tracks.push(track);
            }
        }
    }

    (album_title, tracks)
}

/// Parses the user profile from `guide` (TV mode) response
pub fn parse_tv_user_profile(val: &Value) -> Option<UserProfile> {
    let guide_account = val
        .pointer("/topbar/guideSectionRenderer/items/0/guideAccountEntryRenderer")
        .or_else(|| val.pointer("/items/0/guideAccountEntryRenderer"))?;

    let name = guide_account
        .pointer("/title/simpleText")
        .or_else(|| guide_account.pointer("/title/runs/0/text"))
        .and_then(|v| v.as_str())
        .unwrap_or("Usuário")
        .to_string();

    let avatar_url = guide_account
        .pointer("/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(UserProfile {
        name,
        handle: String::new(),
        avatar_url,
    })
}

/// Parses the user profile from `account/account_menu` (Web mode) or `guide` (TV mode) response
pub fn parse_user_profile(val: &Value) -> Option<UserProfile> {
    if let Some(prof) = parse_tv_user_profile(val) {
        return Some(prof);
    }

    let header = val
        .pointer("/actions/0/openPopupAction/popup/multiPageMenuRenderer/header/activeAccountHeaderRenderer")
        .or_else(|| val.pointer("/header/activeAccountHeaderRenderer"))?;

    let name = header
        .pointer("/accountName/runs/0/text")
        .or_else(|| header.pointer("/accountName/simpleText"))
        .and_then(|v| v.as_str())
        .unwrap_or("Usuário")
        .to_string();

    let handle = header
        .pointer("/channelHandle/runs/0/text")
        .or_else(|| header.pointer("/channelHandle/simpleText"))
        .or_else(|| header.pointer("/email/runs/0/text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let avatar_url = header
        .pointer("/accountPhoto/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(UserProfile {
        name,
        handle,
        avatar_url,
    })
}

/// Parses user's playlists from `FEmusic_liked_playlists` or library browse response
pub fn parse_library_playlists(val: &Value) -> Vec<PlaylistSummary> {
    let mut playlists = Vec::new();

    // 1. Check TVHTML5 tvBrowseRenderer tabs
    if let Some(tabs) = val
        .pointer("/contents/tvBrowseRenderer/content/tvSecondaryNavRenderer/sections/0/tvSecondaryNavSectionRenderer/tabs")
        .and_then(|v| v.as_array())
    {
        for tab in tabs {
            let tr = tab.get("tabRenderer");
            let is_selected = tr.and_then(|t| t.get("selected")).and_then(|s| s.as_bool()).unwrap_or(false);
            let title = tr.and_then(|t| t.get("title")).and_then(|s| s.as_str()).unwrap_or("");
            if is_selected || title.eq_ignore_ascii_case("Playlists") {
                if let Some(items) = tr.and_then(|t| t.pointer("/content/tvSurfaceContentRenderer/content/gridRenderer/items")).and_then(|v| v.as_array()) {
                    for it in items {
                        if let Some(tile) = it.get("tileRenderer") {
                            if let Some(pl) = parse_tv_tile_playlist(tile) {
                                if pl.id != "VLLM" && pl.id != "LM" && pl.id != "FEmusic_liked_videos" && !pl.id.is_empty() {
                                    playlists.push(pl);
                                }
                            }
                        }
                    }
                    if !playlists.is_empty() {
                        return playlists;
                    }
                }
            }
        }
    }

    let mut raw_items = Vec::new();

    let items_array = val
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .or_else(|| val.pointer("/contents/sectionListRenderer/contents"));

    if let Some(sections) = items_array.and_then(|v| v.as_array()) {
        for sec in sections {
            if let Some(items) = sec
                .pointer("/gridRenderer/items")
                .and_then(|v| v.as_array())
            {
                raw_items.extend(items.clone());
            } else if let Some(items) = sec
                .pointer("/itemSectionRenderer/contents/0/gridRenderer/items")
                .and_then(|v| v.as_array())
            {
                raw_items.extend(items.clone());
            } else if let Some(items) = sec
                .pointer("/musicShelfRenderer/contents")
                .and_then(|v| v.as_array())
            {
                raw_items.extend(items.clone());
            }
        }
    }

    // Direct grid fallback
    if raw_items.is_empty() {
        if let Some(items) = val.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/gridRenderer/items").and_then(|v| v.as_array()) {
            raw_items.extend(items.clone());
        }
    }

    for item in raw_items {
        if let Some(two_row) = item.get("musicTwoRowItemRenderer") {
            if let Some(playlist) = parse_two_row_playlist(two_row) {
                if playlist.id != "VLLM"
                    && playlist.id != "FEmusic_liked_videos"
                    && !playlist.id.is_empty()
                {
                    playlists.push(playlist);
                }
            }
        } else if let Some(resp_item) = item.get("musicResponsiveListItemRenderer") {
            if let Some(playlist) = parse_responsive_playlist(resp_item) {
                if playlist.id != "VLLM"
                    && playlist.id != "FEmusic_liked_videos"
                    && !playlist.id.is_empty()
                {
                    playlists.push(playlist);
                }
            }
        }
    }

    playlists
}

fn parse_two_row_playlist(renderer: &Value) -> Option<PlaylistSummary> {
    let browse_id = renderer
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .or_else(|| renderer.pointer("/title/runs/0/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(|v| v.as_str())?
        .to_string();

    let title = renderer
        .pointer("/title/runs/0/text")
        .and_then(|v| v.as_str())?
        .to_string();

    // Skip "New playlist" button/renderer
    if title.to_lowercase().contains("nova playlist")
        || title.to_lowercase().contains("new playlist")
    {
        return None;
    }

    let mut subtitle = String::new();
    let mut count_text = String::new();

    if let Some(runs) = renderer
        .pointer("/subtitle/runs")
        .and_then(|v| v.as_array())
    {
        for r in runs {
            if let Some(text) = r.get("text").and_then(|t| t.as_str()) {
                let s = text.trim();
                if s.is_empty() || s == "•" {
                    continue;
                }
                if s.contains("música")
                    || s.contains("song")
                    || s.contains("faixa")
                    || s.chars().all(|c| c.is_ascii_digit())
                {
                    count_text = s.to_string();
                } else if subtitle.is_empty() {
                    subtitle = s.to_string();
                }
            }
        }
    }

    let thumbnail_url = renderer
        .pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(PlaylistSummary {
        id: browse_id,
        title,
        subtitle,
        count_text,
        thumbnail_url,
    })
}

fn parse_responsive_playlist(renderer: &Value) -> Option<PlaylistSummary> {
    let browse_id = renderer
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .or_else(|| renderer.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(|v| v.as_str())?
        .to_string();

    let flex_columns = renderer.get("flexColumns")?.as_array()?;
    if flex_columns.is_empty() {
        return None;
    }

    let title_runs = flex_columns[0]
        .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?
        .as_array()?;
    let title = title_runs
        .iter()
        .map(|r| r.get("text").and_then(|t| t.as_str()).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("");

    if title.to_lowercase().contains("nova playlist")
        || title.to_lowercase().contains("new playlist")
    {
        return None;
    }

    let mut subtitle = String::new();
    let mut count_text = String::new();

    if flex_columns.len() > 1 {
        if let Some(runs) = flex_columns[1]
            .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")
            .and_then(|v| v.as_array())
        {
            for r in runs {
                if let Some(text) = r.get("text").and_then(|t| t.as_str()) {
                    let s = text.trim();
                    if s.is_empty() || s == "•" {
                        continue;
                    }
                    if s.contains("música")
                        || s.contains("song")
                        || s.contains("faixa")
                        || s.chars().all(|c| c.is_ascii_digit())
                    {
                        count_text = s.to_string();
                    } else if subtitle.is_empty() {
                        subtitle = s.to_string();
                    }
                }
            }
        }
    }

    let thumbnail_url = renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(PlaylistSummary {
        id: browse_id,
        title,
        subtitle,
        count_text,
        thumbnail_url,
    })
}

/// Parses a playlist summary from a TVHTML5 `tileRenderer`
pub fn parse_tv_tile_playlist(tile: &Value) -> Option<PlaylistSummary> {
    let raw_id = tile
        .pointer("/onSelectCommand/browseEndpoint/browseId")
        .or_else(|| tile.pointer("/metadata/tileMetadataRenderer/title/runs/0/navigationEndpoint/browseEndpoint/browseId"))
        .or_else(|| tile.pointer("/onLongPressCommand/showMenuCommand/title/runs/0/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(|v| v.as_str())?;

    let browse_id = if raw_id.starts_with("VL") {
        raw_id[2..].to_string()
    } else {
        raw_id.to_string()
    };

    let title = tile
        .pointer("/metadata/tileMetadataRenderer/title/runs/0/text")
        .or_else(|| tile.pointer("/metadata/tileMetadataRenderer/title/simpleText"))
        .or_else(|| tile.pointer("/onLongPressCommand/showMenuCommand/title/runs/0/text"))
        .and_then(|v| v.as_str())?
        .to_string();

    let mut subtitle = tile
        .pointer("/onLongPressCommand/showMenuCommand/subtitle/simpleText")
        .or_else(|| tile.pointer("/onLongPressCommand/showMenuCommand/subtitle/runs/0/text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut count_text = String::new();

    if let Some(lines) = tile
        .pointer("/metadata/tileMetadataRenderer/lines")
        .and_then(|v| v.as_array())
    {
        for line in lines {
            if let Some(items) = line
                .pointer("/lineRenderer/items")
                .and_then(|v| v.as_array())
            {
                for item in items {
                    let txt = item
                        .pointer("/lineItemRenderer/text/simpleText")
                        .or_else(|| item.pointer("/lineItemRenderer/text/runs/0/text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    let lower = txt.to_lowercase();
                    if lower.contains("track")
                        || lower.contains("song")
                        || lower.contains("faixa")
                        || lower.contains("música")
                        || lower.contains("musica")
                    {
                        count_text = txt.to_string();
                    } else if subtitle.is_empty()
                        && !lower.contains("playlist")
                        && !lower.contains("auto playlist")
                    {
                        subtitle = txt.to_string();
                    }
                }
            }
        }
    }

    let thumbnail_url = tile
        .pointer("/header/tileHeaderRenderer/thumbnail/thumbnails")
        .or_else(|| tile.pointer("/onLongPressCommand/showMenuCommand/thumbnail/thumbnails"))
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(PlaylistSummary {
        id: browse_id,
        title,
        subtitle,
        count_text,
        thumbnail_url,
    })
}

/// Parses a track from a TVHTML5 `tileRenderer`
pub fn parse_tv_tile_track(tile: &Value) -> Option<Track> {
    let video_id = tile
        .pointer("/contentId")
        .or_else(|| tile.pointer("/onSelectCommand/watchEndpoint/videoId"))
        .or_else(|| tile.pointer("/metadata/tileMetadataRenderer/title/runs/0/navigationEndpoint/watchEndpoint/videoId"))
        .or_else(|| tile.pointer("/onLongPressCommand/showMenuCommand/contentId"))
        .or_else(|| tile.pointer("/onLongPressCommand/showMenuCommand/menu/menuRenderer/items/0/menuNavigationItemRenderer/navigationEndpoint/watchEndpoint/videoId"))
        .and_then(|v| v.as_str())?
        .to_string();

    let title = tile
        .pointer("/metadata/tileMetadataRenderer/title/runs/0/text")
        .or_else(|| tile.pointer("/metadata/tileMetadataRenderer/title/simpleText"))
        .or_else(|| tile.pointer("/onLongPressCommand/showMenuCommand/title/runs/0/text"))
        .and_then(|v| v.as_str())?
        .to_string();

    let mut artist = "Artista Desconhecido".to_string();
    let mut album = String::new();

    if let Some(lines) = tile
        .pointer("/metadata/tileMetadataRenderer/lines")
        .and_then(|v| v.as_array())
    {
        for (i, line) in lines.iter().enumerate() {
            if let Some(items) = line
                .pointer("/lineRenderer/items")
                .and_then(|v| v.as_array())
            {
                for item in items {
                    let txt = item
                        .pointer("/lineItemRenderer/text/simpleText")
                        .or_else(|| item.pointer("/lineItemRenderer/text/runs/0/text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    if i == 0 && !txt.is_empty() {
                        artist = txt.to_string();
                    } else if i > 0
                        && !is_view_count(txt)
                        && !is_category_label(txt)
                        && !is_duration_str(txt)
                        && !txt.is_empty()
                    {
                        if album.is_empty() {
                            album = txt.to_string();
                        }
                    }
                }
            }
        }
    }

    if artist == "Artista Desconhecido" {
        if let Some(sub) = tile
            .pointer("/onLongPressCommand/showMenuCommand/subtitle/simpleText")
            .or_else(|| tile.pointer("/onLongPressCommand/showMenuCommand/subtitle/runs/0/text"))
            .and_then(|t| t.as_str())
        {
            if !sub.is_empty() {
                artist = sub.to_string();
            }
        }
    }

    let mut duration_secs = 0;
    if let Some(overlays) = tile
        .pointer("/header/tileHeaderRenderer/thumbnailOverlays")
        .and_then(|v| v.as_array())
    {
        for ov in overlays {
            if let Some(dur_str) = ov
                .pointer("/thumbnailOverlayTimeStatusRenderer/text/simpleText")
                .or_else(|| ov.pointer("/thumbnailOverlayTimeStatusRenderer/text/runs/0/text"))
                .and_then(|t| t.as_str())
            {
                let s = parse_duration_to_secs(dur_str);
                if s > 0 {
                    duration_secs = s;
                    break;
                }
            }
        }
    }

    let thumbnail_url = tile
        .pointer("/header/tileHeaderRenderer/thumbnail/thumbnails")
        .or_else(|| tile.pointer("/onLongPressCommand/showMenuCommand/thumbnail/thumbnails"))
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    let music_video_id = explicit_music_video_id(tile, &video_id);
    Some(
        Track::new(video_id, title, artist, album, duration_secs, thumbnail_url)
            .with_music_video_id(music_video_id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration_to_secs("04:18"), 258);
        assert_eq!(parse_duration_to_secs("01:02:15"), 3735);
        assert_eq!(parse_duration_to_secs("45"), 45);
    }

    #[test]
    fn test_parse_user_profile() {
        let raw = json!({
            "actions": [
                {
                    "openPopupAction": {
                        "popup": {
                            "multiPageMenuRenderer": {
                                "header": {
                                    "activeAccountHeaderRenderer": {
                                        "accountName": {
                                            "runs": [{ "text": "Jean Bitencourt" }]
                                        },
                                        "channelHandle": {
                                            "runs": [{ "text": "@jeanbitencourt" }]
                                        },
                                        "accountPhoto": {
                                            "thumbnails": [
                                                { "url": "https://lh3.googleusercontent.com/avatar_small.jpg", "width": 48 },
                                                { "url": "https://lh3.googleusercontent.com/avatar_large.jpg", "width": 128 }
                                            ]
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            ]
        });

        let profile = parse_user_profile(&raw).expect("profile should parse");
        assert_eq!(profile.name, "Jean Bitencourt");
        assert_eq!(profile.handle, "@jeanbitencourt");
        assert_eq!(
            profile.avatar_url,
            "https://lh3.googleusercontent.com/avatar_large.jpg"
        );
    }

    #[test]
    fn test_parse_library_playlists() {
        let raw = json!({
            "contents": {
                "singleColumnBrowseResultsRenderer": {
                    "tabs": [
                        {
                            "tabRenderer": {
                                "content": {
                                    "sectionListRenderer": {
                                        "contents": [
                                            {
                                                "gridRenderer": {
                                                    "items": [
                                                        {
                                                            "musicTwoRowItemRenderer": {
                                                                "title": { "runs": [{ "text": "Músicas que você gostou" }] },
                                                                "navigationEndpoint": {
                                                                    "browseEndpoint": { "browseId": "VLLM" }
                                                                }
                                                            }
                                                        },
                                                        {
                                                            "musicTwoRowItemRenderer": {
                                                                "title": { "runs": [{ "text": "Synthwave & Retrowave" }] },
                                                                "subtitle": {
                                                                    "runs": [
                                                                        { "text": "Playlist" },
                                                                        { "text": " • " },
                                                                        { "text": "45 músicas" }
                                                                    ]
                                                                },
                                                                "navigationEndpoint": {
                                                                    "browseEndpoint": { "browseId": "VLPL_synthwave_123" }
                                                                },
                                                                "thumbnailRenderer": {
                                                                    "musicThumbnailRenderer": {
                                                                        "thumbnail": {
                                                                            "thumbnails": [{ "url": "https://lh3.googleusercontent.com/pl_thumb.jpg" }]
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    ]
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let playlists = parse_library_playlists(&raw);
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].id, "VLPL_synthwave_123");
        assert_eq!(playlists[0].title, "Synthwave & Retrowave");
        assert_eq!(playlists[0].subtitle, "Playlist");
        assert_eq!(playlists[0].count_text, "45 músicas");
        assert_eq!(
            playlists[0].thumbnail_url,
            "https://lh3.googleusercontent.com/pl_thumb.jpg"
        );
    }

    #[test]
    fn test_parse_tv_user_profile() {
        let raw = json!({
            "topbar": {
                "guideSectionRenderer": {
                    "items": [
                        {
                            "guideAccountEntryRenderer": {
                                "title": { "simpleText": "Jean Bitencourt" },
                                "thumbnail": {
                                    "thumbnails": [
                                        { "url": "https://yt3.ggpht.com/avatar_88.jpg", "width": 88, "height": 88 }
                                    ]
                                }
                            }
                        }
                    ]
                }
            }
        });

        let profile = parse_user_profile(&raw).expect("tv profile should parse");
        assert_eq!(profile.name, "Jean Bitencourt");
        assert_eq!(profile.avatar_url, "https://yt3.ggpht.com/avatar_88.jpg");
    }

    #[test]
    fn test_parse_tv_library_playlists() {
        let raw = json!({
            "contents": {
                "tvBrowseRenderer": {
                    "content": {
                        "tvSecondaryNavRenderer": {
                            "sections": [
                                {
                                    "tvSecondaryNavSectionRenderer": {
                                        "tabs": [
                                            {
                                                "tabRenderer": {
                                                    "title": "Playlists",
                                                    "selected": true,
                                                    "content": {
                                                        "tvSurfaceContentRenderer": {
                                                            "content": {
                                                                "gridRenderer": {
                                                                    "items": [
                                                                        {
                                                                            "tileRenderer": {
                                                                                "onSelectCommand": {
                                                                                    "browseEndpoint": { "browseId": "VLLM" }
                                                                                },
                                                                                "metadata": {
                                                                                    "tileMetadataRenderer": {
                                                                                        "title": { "runs": [{ "text": "Liked Music" }] }
                                                                                    }
                                                                                }
                                                                            }
                                                                        },
                                                                        {
                                                                            "tileRenderer": {
                                                                                "onSelectCommand": {
                                                                                    "browseEndpoint": { "browseId": "VLPL_rock_123" }
                                                                                },
                                                                                "metadata": {
                                                                                    "tileMetadataRenderer": {
                                                                                        "title": { "runs": [{ "text": "Rock Classics" }] },
                                                                                        "lines": [
                                                                                            {
                                                                                                "lineRenderer": {
                                                                                                    "items": [
                                                                                                        { "lineItemRenderer": { "text": { "simpleText": "Jean Bitencourt" } } }
                                                                                                    ]
                                                                                                }
                                                                                            },
                                                                                            {
                                                                                                "lineRenderer": {
                                                                                                    "items": [
                                                                                                        { "lineItemRenderer": { "text": { "simpleText": "Playlist • 50 tracks" } } }
                                                                                                    ]
                                                                                                }
                                                                                            }
                                                                                        ]
                                                                                    }
                                                                                },
                                                                                "header": {
                                                                                    "tileHeaderRenderer": {
                                                                                        "thumbnail": {
                                                                                            "thumbnails": [{ "url": "https://yt3.ggpht.com/rock_thumb.jpg" }]
                                                                                        }
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                                    ]
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        ]
                                    }
                                }
                            ]
                        }
                    }
                }
            }
        });

        let playlists = parse_library_playlists(&raw);
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].id, "PL_rock_123");
        assert_eq!(playlists[0].title, "Rock Classics");
        assert_eq!(playlists[0].subtitle, "Jean Bitencourt");
        assert_eq!(playlists[0].count_text, "Playlist • 50 tracks");
        assert_eq!(
            playlists[0].thumbnail_url,
            "https://yt3.ggpht.com/rock_thumb.jpg"
        );
    }

    #[test]
    fn test_parse_tv_tile_track() {
        let raw = json!({
            "tileRenderer": {
                "contentId": "track12345",
                "metadata": {
                    "tileMetadataRenderer": {
                        "title": { "runs": [{ "text": "The Trooper" }] },
                        "lines": [
                            {
                                "lineRenderer": {
                                    "items": [
                                        { "lineItemRenderer": { "text": { "simpleText": "Iron Maiden" } } }
                                    ]
                                }
                            },
                            {
                                "lineRenderer": {
                                    "items": [
                                        { "lineItemRenderer": { "text": { "simpleText": "Piece of Mind" } } }
                                    ]
                                }
                            }
                        ]
                    }
                },
                "header": {
                    "tileHeaderRenderer": {
                        "thumbnail": {
                            "thumbnails": [{ "url": "https://i.ytimg.com/vi/track12345/hqdefault.jpg" }]
                        },
                        "thumbnailOverlays": [
                            {
                                "thumbnailOverlayTimeStatusRenderer": {
                                    "text": { "simpleText": "4:11" }
                                }
                            }
                        ]
                    }
                }
            }
        });

        let track = parse_tv_tile_track(&raw["tileRenderer"]).expect("tv track should parse");
        assert_eq!(track.video_id, "track12345");
        assert_eq!(track.title, "The Trooper");
        assert_eq!(track.artist, "Iron Maiden");
        assert_eq!(track.album, "Piece of Mind");
        assert_eq!(track.duration_secs, 251);
        assert_eq!(
            track.thumbnail_url,
            "https://i.ytimg.com/vi/track12345/hqdefault.jpg"
        );
    }

    #[test]
    fn test_parse_music_responsive_item() {
        let raw = json!({
            "playlistItemData": {
                "videoId": "abc12345"
            },
            "flexColumns": [
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Nightcall" }
                            ]
                        }
                    }
                },
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Kavinsky" },
                                { "text": " • " },
                                { "text": "OutRun" },
                                { "text": " • " },
                                { "text": "04:18" }
                            ]
                        }
                    }
                }
            ],
            "thumbnail": {
                "musicThumbnailRenderer": {
                    "thumbnail": {
                        "thumbnails": [
                            { "url": "https://lh3.googleusercontent.com/test.jpg" }
                        ]
                    }
                }
            }
        });

        let track = parse_music_responsive_item(&raw).unwrap();
        assert_eq!(track.video_id, "abc12345");
        assert_eq!(track.title, "Nightcall");
        assert_eq!(track.artist, "Kavinsky");
        assert_eq!(track.album, "OutRun");
        assert_eq!(track.duration_secs, 258);
        assert_eq!(track.duration_text, "04:18");
    }

    #[test]
    fn test_parse_album_item() {
        let raw = json!({
            "navigationEndpoint": {
                "browseEndpoint": {
                    "browseId": "MPREb_12345"
                }
            },
            "flexColumns": [
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Random Access Memories" }
                            ]
                        }
                    }
                },
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Álbum" },
                                { "text": " • " },
                                { "text": "Daft Punk" },
                                { "text": " • " },
                                { "text": "2013" }
                            ]
                        }
                    }
                }
            ],
            "thumbnail": {
                "musicThumbnailRenderer": {
                    "thumbnail": {
                        "thumbnails": [
                            { "url": "https://lh3.googleusercontent.com/album.jpg" }
                        ]
                    }
                }
            }
        });

        let album = parse_album_item(&raw).unwrap();
        assert_eq!(album.browse_id, "MPREb_12345");
        assert_eq!(album.title, "Random Access Memories");
        assert_eq!(album.artist, "Daft Punk");
        assert_eq!(album.year, "2013");
        assert_eq!(
            album.thumbnail_url,
            "https://lh3.googleusercontent.com/album.jpg"
        );
    }

    #[test]
    fn test_parse_artist_item() {
        let raw = json!({
            "navigationEndpoint": {
                "browseEndpoint": {
                    "browseId": "UC_artist_123"
                }
            },
            "flexColumns": [
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Daft Punk" }
                            ]
                        }
                    }
                },
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Artista" },
                                { "text": " • " },
                                { "text": "68,3 mi ouvintes mensais" }
                            ]
                        }
                    }
                }
            ],
            "thumbnail": {
                "musicThumbnailRenderer": {
                    "thumbnail": {
                        "thumbnails": [
                            { "url": "https://lh3.googleusercontent.com/artist.jpg" }
                        ]
                    }
                }
            }
        });

        let artist = parse_artist_item(&raw).unwrap();
        assert_eq!(artist.browse_id, "UC_artist_123");
        assert_eq!(artist.name, "Daft Punk");
        assert_eq!(artist.subscribers_text, "68,3 mi ouvintes mensais");
        assert_eq!(
            artist.thumbnail_url,
            "https://lh3.googleusercontent.com/artist.jpg"
        );
    }

    #[test]
    fn test_is_view_count() {
        assert!(is_view_count("17 mi de visualização"));
        assert!(is_view_count("326 mil visualizações"));
        assert!(is_view_count("1.2M views"));
        assert!(is_view_count("500 views"));
        assert!(is_view_count("100 ouvintes"));
        assert!(is_view_count("15 mi de reproduções"));
        assert!(!is_view_count("Somewhere in Time"));
        assert!(!is_view_count("Rebirth"));
        assert!(!is_view_count("Iron Maiden"));
    }

    #[test]
    fn test_parse_playlist_panel_video_ignores_view_count_as_album() {
        let raw = json!({
            "videoId": "vid_xyz_123",
            "title": {
                "runs": [
                    { "text": "Stranger in a Strange Land" }
                ]
            },
            "longBylineText": {
                "runs": [
                    {
                        "text": "Iron Maiden",
                        "navigationEndpoint": {
                            "browseEndpoint": {
                                "browseEndpointContextSupportedConfigs": {
                                    "browseEndpointContextMusicConfig": {
                                        "pageType": "MUSIC_PAGE_TYPE_ARTIST"
                                    }
                                }
                            }
                        }
                    },
                    { "text": " • " },
                    { "text": "15 mi de visualização" }
                ]
            },
            "lengthText": {
                "runs": [
                    { "text": "05:47" }
                ]
            },
            "thumbnail": {
                "thumbnails": [
                    { "url": "https://lh3.googleusercontent.com/track.jpg" }
                ]
            }
        });

        let track = parse_playlist_panel_video(&raw).unwrap();
        assert_eq!(track.video_id, "vid_xyz_123");
        assert_eq!(track.title, "Stranger in a Strange Land");
        assert_eq!(track.artist, "Iron Maiden");
        assert_eq!(track.album, "");
        assert_eq!(track.duration_secs, 347);
        assert_eq!(track.duration_text, "05:47");
    }

    #[test]
    fn test_extract_and_parse_continuations() {
        let raw_browse = json!({
            "contents": {
                "tvBrowseRenderer": {
                    "content": {
                        "tvSurfaceContentRenderer": {
                            "content": {
                                "twoColumnRenderer": {
                                    "rightColumn": {
                                        "playlistVideoListRenderer": {
                                            "contents": [],
                                            "continuations": [
                                                {
                                                    "nextContinuationData": {
                                                        "continuation": "token_page_2"
                                                    }
                                                }
                                            ]
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        assert_eq!(
            extract_browse_continuation_token(&raw_browse),
            Some("token_page_2".to_string())
        );

        let raw_continuation = json!({
            "continuationContents": {
                "playlistVideoListContinuation": {
                    "contents": [
                        {
                            "tileRenderer": {
                                "contentId": "track_cont_1",
                                "metadata": {
                                    "tileMetadataRenderer": {
                                        "title": { "simpleText": "Aces High" },
                                        "lines": [
                                            {
                                                "lineRenderer": {
                                                    "items": [
                                                        { "lineItemRenderer": { "text": { "simpleText": "Iron Maiden" } } }
                                                    ]
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ],
                    "continuations": [
                        {
                            "nextContinuationData": {
                                "continuation": "token_page_3"
                            }
                        }
                    ]
                }
            }
        });

        let (tracks, next_tok) = parse_continuation_response(&raw_continuation);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].video_id, "track_cont_1");
        assert_eq!(tracks[0].title, "Aces High");
        assert_eq!(tracks[0].artist, "Iron Maiden");
        assert_eq!(next_tok, Some("token_page_3".to_string()));
    }
}
