use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, LineGauge, Paragraph};
use ratatui::Frame;

use crate::audio::{PlaybackStatus, PlayerState};
use crate::theme::Theme;

pub fn render_playerbar(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    player: &PlayerState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.unfocused_border_style())
        .style(theme.default_block_style());

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 {
        return;
    }

    // Split horizontally: Left (Cover Icon / Art) | Right (Info & Controls)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(8), Constraint::Min(20)])
        .split(inner);

    // Left: Music Note Icon / Thumbnail representation
    let icon_lines = vec![
        Line::from(Span::styled("  ♫ ", theme.title_style())),
        Line::from(Span::styled(" [YTM]", theme.muted_style())),
    ];
    let icon_p = Paragraph::new(icon_lines).alignment(Alignment::Center);
    f.render_widget(icon_p, chunks[0]);

    // Right: Two rows: Row 0 (Title / Artist / Codec) and Row 1 (Progress + Volume)
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(chunks[1]);

    // Row 0: Title — Artist [Opus 160kbps • Background Daemon ON]
    let (title, artist) = if let Some(t) = &player.current_track {
        (t.title.as_str(), t.artist.as_str())
    } else {
        ("Nenhuma faixa em reprodução", "Selecione uma faixa e pressione Enter")
    };

    let status_icon = match player.status {
        PlaybackStatus::Playing => "▶ ",
        PlaybackStatus::Paused => "⏸ ",
        PlaybackStatus::Buffering => "⏳ ",
        PlaybackStatus::Stopped => "■ ",
    };

    let like_str = if player.is_liked { " ♥" } else { "" };

    let top_line = Line::from(vec![
        Span::styled(status_icon, theme.accent_style().add_modifier(Modifier::BOLD)),
        Span::styled(title, theme.title_style()),
        Span::styled(like_str, theme.liked_style()),
        Span::styled(" — ", theme.muted_style()),
        Span::styled(artist, theme.text_style()),
        Span::styled("  [Opus 160kbps • Daemon ON]", theme.muted_style()),
    ]);
    f.render_widget(Paragraph::new(top_line), right_chunks[0]);

    // Row 1: 01:42 ━━━●──────── 04:18  🔊 Vol: 85%
    let cur_secs = player.position_secs as u32;
    let dur_secs = player.duration_secs as u32;

    let cur_text = format!("{:02}:{:02}", cur_secs / 60, cur_secs % 60);
    let dur_text = format!("{:02}:{:02}", dur_secs / 60, dur_secs % 60);

    let progress_ratio = if dur_secs > 0 {
        (player.position_secs / player.duration_secs).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Subdivide Row 1 into: Current Time | Progress LineGauge | Total Time | Volume
    let progress_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(6),
            Constraint::Min(15),
            Constraint::Length(7),
            Constraint::Length(15),
        ])
        .split(right_chunks[1]);

    f.render_widget(Paragraph::new(cur_text).style(theme.muted_style()), progress_chunks[0]);

    let gauge = LineGauge::default()
        .filled_style(theme.accent_style().add_modifier(Modifier::BOLD))
        .unfilled_style(theme.muted_style())
        .ratio(progress_ratio)
        .line_set(ratatui::symbols::line::THICK);
    f.render_widget(gauge, progress_chunks[1]);

    f.render_widget(Paragraph::new(dur_text).style(theme.muted_style()), progress_chunks[2]);

    let vol_text = format!("🔊 Vol: {:>2}%", player.volume);
    f.render_widget(Paragraph::new(vol_text).style(theme.text_style()).alignment(Alignment::Right), progress_chunks[3]);
}
