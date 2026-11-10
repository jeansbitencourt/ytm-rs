use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use ratatui_image::StatefulImage;

use crate::app::{ActivePanel, App, ContentView};

pub mod header;
pub mod modals;
pub mod playerbar;
pub mod sidebar;
pub mod tracklist;

pub use header::{get_search_filter_button_at, get_search_filter_button_at_for_language};

pub fn render_ui(f: &mut Frame, app: &mut App) {
    let size = f.area();
    let theme = app.theme.clone();

    // Outer container layout: Header (4) | Content (Min 8) | Player (5)
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(5),
        ])
        .split(size);

    app.playerbar_area = main_chunks[2];

    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(main_chunks[0]);
    app.header_search_area = header_chunks[0];

    // 1. Header (Search Box on Left + Profile/Status on Right)
    header::render_header(f, header_chunks[0], header_chunks[1], app);

    // 2. Middle Content (Sidebar 28% + Main View 72%)
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
        .split(main_chunks[1]);

    // Split left side into Library (top) and Album Cover (bottom)
    let sidebar_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(content_chunks[0]);

    app.sidebar_area = sidebar_chunks[0];

    let counts = (
        app.liked_tracks.len(),
        app.history_tracks.len(),
        app.player_state.queue.len(),
    );

    let is_sidebar_focused = app.active_panel == ActivePanel::Sidebar;
    sidebar::render_sidebar(
        f,
        sidebar_chunks[0],
        &theme,
        &mut app.sidebar_state,
        is_sidebar_focused,
        counts,
        &app.user_playlists,
        &app.i18n,
    );

    render_cover_art(
        f,
        sidebar_chunks[1],
        &theme,
        &mut app.cover_protocol,
        app.player_state.current_track.is_some(),
        app.picker.font_size,
        &app.i18n,
    );

    let is_tracklist_focused = app.active_panel == ActivePanel::Tracklist;
    let current_vid = app
        .player_state
        .current_track
        .as_ref()
        .map(|t| t.video_id.as_str());
    let is_playing = app.player_state.status == crate::audio::PlaybackStatus::Playing;
    app.tracklist_area = content_chunks[1];

    let category_title = app.localized_category_title();
    match app.content_view {
        ContentView::Tracks => {
            tracklist::render_tracklist(
                f,
                content_chunks[1],
                &theme,
                &category_title,
                &app.displayed_tracks,
                current_vid,
                is_playing,
                &mut app.tracklist_state,
                is_tracklist_focused,
                &mut app.thumb_protocols,
                app.is_loading_tracks,
                &app.i18n,
            );
        }
        ContentView::Albums => {
            tracklist::render_album_list(
                f,
                content_chunks[1],
                &theme,
                &category_title,
                &app.search_results_albums,
                &mut app.album_table_state,
                is_tracklist_focused,
                &mut app.thumb_protocols,
                &app.i18n,
            );
        }
        ContentView::Artists => {
            tracklist::render_artist_list(
                f,
                content_chunks[1],
                &theme,
                &category_title,
                &app.search_results_artists,
                &mut app.artist_table_state,
                is_tracklist_focused,
                &mut app.thumb_protocols,
                &app.i18n,
            );
        }
    }

    // 3. Player Bar
    playerbar::render_playerbar(f, main_chunks[2], &theme, &app.player_state, &app.i18n);

    // 4. Help Modal if toggled
    if app.show_help {
        modals::render_help_modal(f, size, app);
    }

    // 5. Auth Modal if toggled
    if app.show_auth_modal {
        modals::render_auth_modal(f, size, app);
    }

    if app.show_language_modal {
        modals::render_language_modal(f, size, app);
    }
}

pub fn compute_cover_art_rect(inner_area: Rect, font_size: (u16, u16)) -> Option<Rect> {
    if inner_area.width < 4 || inner_area.height < 2 {
        return None;
    }

    let (font_w, font_h) = (font_size.0.max(1), font_size.1.max(1));
    let cell_ratio = font_h as f32 / font_w as f32;

    let avail_w = inner_area.width.saturating_sub(2);
    let avail_h = inner_area.height.saturating_sub(2);

    if avail_w < 2 || avail_h < 2 {
        return None;
    }

    let target_w_from_h = ((avail_h as f32) * cell_ratio).round() as u16;

    let (target_w, target_h) = if target_w_from_h <= avail_w {
        (target_w_from_h, avail_h)
    } else {
        let target_h_from_w = ((avail_w as f32) / cell_ratio).round() as u16;
        (avail_w, target_h_from_w.min(avail_h))
    };

    let target_w = target_w.max(1).min(avail_w);
    let target_h = target_h.max(1).min(avail_h);

    let offset_x = (inner_area.width.saturating_sub(target_w)) / 2;
    let offset_y = (inner_area.height.saturating_sub(target_h)) / 2;

    Some(Rect {
        x: inner_area.x + offset_x,
        y: inner_area.y + offset_y,
        width: target_w,
        height: target_h,
    })
}

fn render_cover_art(
    f: &mut Frame,
    area: Rect,
    theme: &crate::theme::Theme,
    cover_protocol: &mut Option<Box<dyn ratatui_image::protocol::StatefulProtocol>>,
    is_playing: bool,
    font_size: (u16, u16),
    i18n: &crate::i18n::I18n,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.unfocused_border_style())
        .title(Span::styled(i18n.t("cover.title"), theme.title_style()))
        .style(theme.default_block_style());

    let inner_area = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    if inner_area.width < 4 || inner_area.height < 2 {
        return;
    }

    if let Some(protocol) = cover_protocol {
        if let Some(target_rect) = compute_cover_art_rect(inner_area, font_size) {
            let image_widget = StatefulImage::new(None);
            f.render_stateful_widget(image_widget, target_rect, protocol);
        }
    } else {
        let placeholder = if is_playing {
            vec![
                Line::from(""),
                Line::from(Span::styled(i18n.t("cover.loading"), theme.muted_style())),
            ]
        } else {
            vec![
                Line::from(""),
                Line::from(Span::styled("♫", theme.title_style())),
                Line::from(Span::styled("YTM-RS", theme.muted_style())),
            ]
        };
        let p = Paragraph::new(placeholder).alignment(Alignment::Center);
        f.render_widget(p, inner_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::innertube::SearchFilter;
    use ratatui::layout::Rect;

    #[test]
    fn test_search_filter_button_hit_detection() {
        let search_area = Rect::new(0, 0, 80, 4);

        assert_eq!(get_search_filter_button_at(30, 3, search_area), None);
        assert_eq!(get_search_filter_button_at(10, 0, search_area), None);

        assert_eq!(
            get_search_filter_button_at(28, 0, search_area),
            Some(SearchFilter::Tracks)
        );
        assert_eq!(
            get_search_filter_button_at(41, 0, search_area),
            Some(SearchFilter::Tracks)
        );

        assert_eq!(
            get_search_filter_button_at(42, 0, search_area),
            Some(SearchFilter::Albums)
        );
        assert_eq!(
            get_search_filter_button_at(53, 0, search_area),
            Some(SearchFilter::Albums)
        );

        assert_eq!(
            get_search_filter_button_at(54, 0, search_area),
            Some(SearchFilter::Artists)
        );
        assert_eq!(
            get_search_filter_button_at(68, 0, search_area),
            Some(SearchFilter::Artists)
        );

        assert_eq!(get_search_filter_button_at(70, 0, search_area), None);
    }

    #[test]
    fn test_compute_cover_art_rect_bounds_and_margins() {
        let inner_area = Rect::new(10, 10, 42, 28);
        let font_size = (7, 16);

        let target = compute_cover_art_rect(inner_area, font_size).expect("should compute rect");

        assert!(target.left() >= inner_area.left());
        assert!(target.top() >= inner_area.top());
        assert!(target.right() <= inner_area.right());
        assert!(target.bottom() <= inner_area.bottom());

        assert!(target.left() > inner_area.left());
        assert!(target.right() < inner_area.right());
        assert!(target.top() > inner_area.top());
        assert!(target.bottom() < inner_area.bottom());

        let w_px = target.width as f32 * font_size.0 as f32;
        let h_px = target.height as f32 * font_size.1 as f32;
        let ratio = w_px / h_px;
        assert!(
            (ratio - 1.0).abs() < 0.15,
            "image should be approximately square in pixels: {}",
            ratio
        );
    }

    #[test]
    fn test_compute_cover_art_rect_various_dimensions() {
        let font_sizes = [(7, 16), (8, 16), (9, 18), (10, 20)];
        let test_areas = [
            Rect::new(0, 0, 80, 20),
            Rect::new(5, 5, 20, 40),
            Rect::new(1, 1, 30, 30),
            Rect::new(0, 0, 44, 30),
        ];

        for &font_size in &font_sizes {
            for &area in &test_areas {
                let target = compute_cover_art_rect(area, font_size).unwrap();
                assert!(target.left() >= area.left());
                assert!(target.top() >= area.top());
                assert!(target.right() <= area.right());
                assert!(target.bottom() <= area.bottom());
                assert!(target.right() < area.right() + 1);
            }
        }

        assert_eq!(compute_cover_art_rect(Rect::new(0, 0, 3, 3), (7, 16)), None);
        assert_eq!(
            compute_cover_art_rect(Rect::new(0, 0, 10, 2), (7, 16)),
            None
        );
    }
}
