use ratatui::layout::{Alignment, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::StatefulImage;
use std::collections::{HashMap, HashSet};

use crate::audio::PlaybackStatus;
use crate::i18n::I18n;
use crate::innertube::{AlbumSummary, ArtistSummary, Track};
use crate::theme::Theme;

fn truncate_str(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else if max_chars > 1 {
        let truncated: String = chars[..max_chars - 1].iter().collect();
        format!("{}…", truncated)
    } else if max_chars == 1 {
        chars[0].to_string()
    } else {
        String::new()
    }
}

pub fn render_tracklist(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    title_text: &str,
    tracks: &[Track],
    current_video_id: Option<&str>,
    playback_status: PlaybackStatus,
    state: &mut ListState,
    is_focused: bool,
    thumb_protocols: &mut HashMap<String, Box<dyn StatefulProtocol>>,
    is_loading: bool,
    i18n: &I18n,
) {
    let border_style = if is_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let title = Line::from(vec![
        Span::styled(
            format!(" {}: {} [2] ", i18n.t("tracklist.title"), title_text),
            theme.title_style(),
        ),
        Span::styled(i18n.t("tracklist.hint"), theme.muted_style()),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title)
        .style(theme.default_block_style());

    let inner_area = block.inner(area);

    if inner_area.width < 15 || inner_area.height < 4 {
        f.render_widget(block, area);
        return;
    }

    if tracks.is_empty() {
        let msg = if is_loading {
            i18n.t("tracklist.loading")
        } else {
            i18n.t("tracklist.empty")
        };
        let empty_msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(msg, theme.muted_style())),
        ])
        .alignment(Alignment::Center)
        .block(block);
        f.render_widget(empty_msg, area);
        return;
    }

    let inner_w = inner_area.width as usize;

    // Build 4-line cards for each track (with balanced top and bottom padding)
    let items: Vec<ListItem> = tracks
        .iter()
        .enumerate()
        .map(|(i, track)| {
            let is_current = current_video_id == Some(&track.video_id);
            let is_selected = state.selected() == Some(i);
            let has_thumb = thumb_protocols.contains_key(&track.video_id);

            let (num_str, num_style) = if is_current {
                match playback_status {
                    PlaybackStatus::Playing => {
                        (format!("▶ {:02} ", i + 1), theme.playing_indicator_style())
                    }
                    PlaybackStatus::Paused => (format!("⏸ {:02} ", i + 1), theme.accent_style()),
                    PlaybackStatus::Buffering => {
                        (format!("⏳ {:02} ", i + 1), theme.accent_style())
                    }
                    PlaybackStatus::Stopped => (format!("■ {:02} ", i + 1), theme.muted_style()),
                }
            } else if is_selected {
                (format!("▸ {:02} ", i + 1), theme.accent_style())
            } else {
                (format!("  {:02} ", i + 1), theme.muted_style())
            };

            let title_style = if is_current {
                theme.playing_indicator_style()
            } else {
                theme.text_style().add_modifier(Modifier::BOLD)
            };

            let like_span = if track.is_liked {
                Span::styled(" ♥", theme.liked_style())
            } else {
                Span::raw("")
            };
            let like_w = if track.is_liked { 2 } else { 0 };

            let dur_str = format!(" {} ", track.duration_text);
            let dur_len = dur_str.chars().count();
            let dur_span = Span::styled(dur_str, theme.muted_style());

            let num_len = num_str.chars().count();
            let left_w = 6 + num_len;
            let right_w = dur_len;
            let available_for_title = inner_w.saturating_sub(left_w + right_w + like_w);

            let disp_title = truncate_str(&track.title, available_for_title);
            let title_chars = disp_title.chars().count();
            let pad_title_w = available_for_title.saturating_sub(title_chars);
            let pad_title = " ".repeat(pad_title_w);

            let thumb_span_top = if has_thumb {
                Span::raw("      ")
            } else {
                Span::styled(" [♫ ] ", theme.muted_style())
            };

            // Line 0: Top padding line
            let line0 = Line::from(vec![Span::raw(" ".repeat(inner_w))]);

            // Line 1: Track title & duration
            let line1 = Line::from(vec![
                thumb_span_top,
                Span::styled(num_str, num_style),
                Span::styled(disp_title, title_style),
                like_span,
                Span::raw(pad_title),
                dur_span,
            ]);

            // Line 2: Artist • Album
            let subtitle = if !track.album.is_empty() && track.album != track.artist {
                format!("{} • {}", track.artist, track.album)
            } else {
                track.artist.clone()
            };

            let avail_sub_w = inner_w.saturating_sub(left_w + 1);
            let disp_sub = truncate_str(&subtitle, avail_sub_w);
            let sub_chars = disp_sub.chars().count();
            let pad_sub = " ".repeat(avail_sub_w.saturating_sub(sub_chars));

            let sub_style = if is_selected {
                theme.text_style()
            } else {
                theme.muted_style()
            };

            let thumb_span_bottom = if has_thumb {
                Span::raw("      ")
            } else {
                Span::styled(" [  ] ", theme.muted_style())
            };

            let num_pad = " ".repeat(num_len);
            let line2 = Line::from(vec![
                thumb_span_bottom,
                Span::raw(num_pad),
                Span::styled(disp_sub, sub_style),
                Span::raw(pad_sub),
                Span::raw(" "),
            ]);

            // Line 3: Bottom padding line
            let line3 = Line::from(vec![Span::raw(" ".repeat(inner_w))]);

            ListItem::new(vec![line0, line1, line2, line3])
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(theme.selected_row_style())
        .highlight_symbol("");

    f.render_stateful_widget(list, area, state);

    // Render thumbnail images on top of visible cards
    let offset = state.offset();
    let visible_card_count = (inner_area.height / 4) as usize;
    let mut seen_ids = HashSet::new();

    for i in 0..visible_card_count {
        let idx = offset + i;
        if idx >= tracks.len() {
            break;
        }
        let card_y = inner_area.y + (i as u16 * 4);
        // Safety guard: ensure thumbnail never touches or overflows the bottom border
        if card_y + 4 > inner_area.bottom() {
            break;
        }

        let track = &tracks[idx];
        if !seen_ids.insert(&track.video_id) {
            continue;
        }

        if let Some(protocol) = thumb_protocols.get_mut(&track.video_id) {
            let thumb_rect = Rect {
                x: inner_area.x + 1,
                y: card_y + 1,
                width: 4,
                height: 2,
            };
            let image_widget = StatefulImage::new(None);
            f.render_stateful_widget(image_widget, thumb_rect, protocol);
        }
    }
}

pub fn render_album_list(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    title_text: &str,
    albums: &[AlbumSummary],
    state: &mut ListState,
    is_focused: bool,
    thumb_protocols: &mut HashMap<String, Box<dyn StatefulProtocol>>,
    i18n: &I18n,
) {
    let border_style = if is_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let title = Line::from(vec![
        Span::styled(
            format!(" {}: {} [2] ", i18n.t("albums.title"), title_text),
            theme.title_style(),
        ),
        Span::styled(i18n.t("albums.hint"), theme.muted_style()),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title)
        .style(theme.default_block_style());

    let inner_area = block.inner(area);

    if inner_area.width < 15 || inner_area.height < 4 {
        f.render_widget(block, area);
        return;
    }

    if albums.is_empty() {
        let empty_msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(i18n.t("albums.empty"), theme.muted_style())),
        ])
        .alignment(Alignment::Center)
        .block(block);
        f.render_widget(empty_msg, area);
        return;
    }

    let inner_w = inner_area.width as usize;

    let items: Vec<ListItem> = albums
        .iter()
        .enumerate()
        .map(|(i, album)| {
            let is_selected = state.selected() == Some(i);
            let has_thumb = thumb_protocols.contains_key(&album.browse_id);

            let (num_str, num_style) = if is_selected {
                (format!("▸ {:02} ", i + 1), theme.accent_style())
            } else {
                (format!("  {:02} ", i + 1), theme.muted_style())
            };

            let title_style = theme.accent_style().add_modifier(Modifier::BOLD);

            let year_str = if !album.year.is_empty() {
                format!(" {} ", album.year)
            } else {
                format!(" {} ", i18n.t("albums.title"))
            };
            let year_len = year_str.chars().count();
            let year_span = Span::styled(year_str, theme.muted_style());

            let left_w = 11;
            let right_w = year_len;
            let available_for_title = inner_w.saturating_sub(left_w + right_w);

            let disp_title = truncate_str(&album.title, available_for_title);
            let title_chars = disp_title.chars().count();
            let pad_title_w = available_for_title.saturating_sub(title_chars);
            let pad_title = " ".repeat(pad_title_w);

            let thumb_span_top = if has_thumb {
                Span::raw("      ")
            } else {
                Span::styled(" [💿] ", theme.muted_style())
            };

            // Line 0: Top padding line
            let line0 = Line::from(vec![Span::raw(" ".repeat(inner_w))]);

            // Line 1: Album title & year
            let line1 = Line::from(vec![
                thumb_span_top,
                Span::styled(num_str, num_style),
                Span::styled(disp_title, title_style),
                Span::raw(pad_title),
                year_span,
            ]);

            // Line 2: Artist name
            let subtitle = if !album.artist.is_empty() {
                format!("{} • {}", album.artist, i18n.t("albums.title").trim())
            } else {
                i18n.t("albums.title")
            };

            let avail_sub_w = inner_w.saturating_sub(left_w + 1);
            let disp_sub = truncate_str(&subtitle, avail_sub_w);
            let sub_chars = disp_sub.chars().count();
            let pad_sub = " ".repeat(avail_sub_w.saturating_sub(sub_chars));

            let sub_style = if is_selected {
                theme.text_style()
            } else {
                theme.muted_style()
            };

            let thumb_span_bottom = if has_thumb {
                Span::raw("      ")
            } else {
                Span::styled(" [  ] ", theme.muted_style())
            };

            let line2 = Line::from(vec![
                thumb_span_bottom,
                Span::raw("     "),
                Span::styled(disp_sub, sub_style),
                Span::raw(pad_sub),
                Span::raw(" "),
            ]);

            // Line 3: Bottom padding line
            let line3 = Line::from(vec![Span::raw(" ".repeat(inner_w))]);

            ListItem::new(vec![line0, line1, line2, line3])
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(theme.selected_row_style())
        .highlight_symbol("");

    f.render_stateful_widget(list, area, state);

    // Render thumbnail images on top of visible cards
    let offset = state.offset();
    let visible_card_count = (inner_area.height / 4) as usize;
    let mut seen_ids = HashSet::new();

    for i in 0..visible_card_count {
        let idx = offset + i;
        if idx >= albums.len() {
            break;
        }
        let card_y = inner_area.y + (i as u16 * 4);
        if card_y + 4 > inner_area.bottom() {
            break;
        }

        let album = &albums[idx];
        if !seen_ids.insert(&album.browse_id) {
            continue;
        }

        if let Some(protocol) = thumb_protocols.get_mut(&album.browse_id) {
            let thumb_rect = Rect {
                x: inner_area.x + 1,
                y: card_y + 1,
                width: 4,
                height: 2,
            };
            let image_widget = StatefulImage::new(None);
            f.render_stateful_widget(image_widget, thumb_rect, protocol);
        }
    }
}

pub fn render_artist_list(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    title_text: &str,
    artists: &[ArtistSummary],
    state: &mut ListState,
    is_focused: bool,
    thumb_protocols: &mut HashMap<String, Box<dyn StatefulProtocol>>,
    i18n: &I18n,
) {
    let border_style = if is_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let title = Line::from(vec![
        Span::styled(
            format!(" {}: {} [2] ", i18n.t("artists.title"), title_text),
            theme.title_style(),
        ),
        Span::styled(i18n.t("artists.hint"), theme.muted_style()),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title)
        .style(theme.default_block_style());

    let inner_area = block.inner(area);

    if inner_area.width < 15 || inner_area.height < 4 {
        f.render_widget(block, area);
        return;
    }

    if artists.is_empty() {
        let empty_msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(i18n.t("artists.empty"), theme.muted_style())),
        ])
        .alignment(Alignment::Center)
        .block(block);
        f.render_widget(empty_msg, area);
        return;
    }

    let inner_w = inner_area.width as usize;

    let items: Vec<ListItem> = artists
        .iter()
        .enumerate()
        .map(|(i, artist)| {
            let is_selected = state.selected() == Some(i);
            let has_thumb = thumb_protocols.contains_key(&artist.browse_id);

            let (num_str, num_style) = if is_selected {
                (format!("▸ {:02} ", i + 1), theme.accent_style())
            } else {
                (format!("  {:02} ", i + 1), theme.muted_style())
            };

            let title_style = theme.accent_style().add_modifier(Modifier::BOLD);

            let left_w = 11;
            let available_for_title = inner_w.saturating_sub(left_w + 1);

            let disp_title = truncate_str(&artist.name, available_for_title);
            let title_chars = disp_title.chars().count();
            let pad_title_w = available_for_title.saturating_sub(title_chars);
            let pad_title = " ".repeat(pad_title_w);

            let thumb_span_top = if has_thumb {
                Span::raw("      ")
            } else {
                Span::styled(" [🎤] ", theme.muted_style())
            };

            // Line 0: Top padding line
            let line0 = Line::from(vec![Span::raw(" ".repeat(inner_w))]);

            // Line 1: Artist name
            let line1 = Line::from(vec![
                thumb_span_top,
                Span::styled(num_str, num_style),
                Span::styled(disp_title, title_style),
                Span::raw(pad_title),
            ]);

            // Line 2: Subscribers / Details
            let subtitle = if !artist.subscribers_text.is_empty() {
                artist.subscribers_text.clone()
            } else {
                i18n.t("artists.title")
            };

            let avail_sub_w = inner_w.saturating_sub(left_w + 1);
            let disp_sub = truncate_str(&subtitle, avail_sub_w);
            let sub_chars = disp_sub.chars().count();
            let pad_sub = " ".repeat(avail_sub_w.saturating_sub(sub_chars));

            let sub_style = if is_selected {
                theme.text_style()
            } else {
                theme.muted_style()
            };

            let thumb_span_bottom = if has_thumb {
                Span::raw("      ")
            } else {
                Span::styled(" [  ] ", theme.muted_style())
            };

            let line2 = Line::from(vec![
                thumb_span_bottom,
                Span::raw("     "),
                Span::styled(disp_sub, sub_style),
                Span::raw(pad_sub),
                Span::raw(" "),
            ]);

            // Line 3: Bottom padding line
            let line3 = Line::from(vec![Span::raw(" ".repeat(inner_w))]);

            ListItem::new(vec![line0, line1, line2, line3])
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(theme.selected_row_style())
        .highlight_symbol("");

    f.render_stateful_widget(list, area, state);

    // Render thumbnail images on top of visible cards
    let offset = state.offset();
    let visible_card_count = (inner_area.height / 4) as usize;
    let mut seen_ids = HashSet::new();

    for i in 0..visible_card_count {
        let idx = offset + i;
        if idx >= artists.len() {
            break;
        }
        let card_y = inner_area.y + (i as u16 * 4);
        if card_y + 4 > inner_area.bottom() {
            break;
        }

        let artist = &artists[idx];
        if !seen_ids.insert(&artist.browse_id) {
            continue;
        }

        if let Some(protocol) = thumb_protocols.get_mut(&artist.browse_id) {
            let thumb_rect = Rect {
                x: inner_area.x + 1,
                y: card_y + 1,
                width: 4,
                height: 2,
            };
            let image_widget = StatefulImage::new(None);
            f.render_stateful_widget(image_widget, thumb_rect, protocol);
        }
    }
}
