use ratatui::layout::{Constraint, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::innertube::{AlbumSummary, ArtistSummary, Track};
use crate::theme::Theme;

pub fn render_tracklist(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    title_text: &str,
    tracks: &[Track],
    current_video_id: Option<&str>,
    is_playing: bool,
    state: &mut TableState,
    is_focused: bool,
) {
    let border_style = if is_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let title = Line::from(vec![
        Span::styled(format!(" Faixas: {} [2] ", title_text), theme.title_style()),
        Span::styled("── [f: Favoritar | a: Add Fila | r: Rádio | q: Esconder] ", theme.muted_style()),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title)
        .style(theme.default_block_style());

    let header_cells = ["#", "TÍTULO", "ARTISTA", "ÁLBUM", "DURAÇÃO"]
        .iter()
        .map(|h| Cell::from(*h).style(theme.muted_style().add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows = tracks.iter().enumerate().map(|(i, track)| {
        let is_current = current_video_id == Some(&track.video_id);
        let num_str = if is_current {
            if is_playing {
                format!("▶ {:02}", i + 1)
            } else {
                format!("⏸ {:02}", i + 1)
            }
        } else {
            format!("  {:02}", i + 1)
        };

        let num_style = if is_current {
            theme.playing_indicator_style()
        } else {
            theme.muted_style()
        };

        let title_style = if is_current {
            theme.playing_indicator_style()
        } else {
            theme.text_style()
        };

        let like_icon = if track.is_liked { " ♥" } else { "" };
        let title_with_like = format!("{}{}", track.title, like_icon);

        let cells = vec![
            Cell::from(num_str).style(num_style),
            Cell::from(title_with_like).style(title_style),
            Cell::from(track.artist.clone()).style(theme.text_style()),
            Cell::from(track.album.clone()).style(theme.muted_style()),
            Cell::from(track.duration_text.clone()).style(theme.muted_style()),
        ];

        Row::new(cells).height(1)
    });

    let widths = [
        Constraint::Length(5),
        Constraint::Percentage(40),
        Constraint::Percentage(25),
        Constraint::Percentage(20),
        Constraint::Length(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .highlight_style(theme.selected_row_style())
        .highlight_symbol("▸ ");

    f.render_stateful_widget(table, area, state);
}

pub fn render_album_list(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    title_text: &str,
    albums: &[AlbumSummary],
    state: &mut TableState,
    is_focused: bool,
) {
    let border_style = if is_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let title = Line::from(vec![
        Span::styled(format!(" Álbuns: {} [2] ", title_text), theme.title_style()),
        Span::styled("── [Enter: Abrir | p: Tocar Álbum | a: Add Fila | q: Esconder] ", theme.muted_style()),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title)
        .style(theme.default_block_style());

    let header_cells = ["#", "ÁLBUM", "ARTISTA", "ANO"]
        .iter()
        .map(|h| Cell::from(*h).style(theme.muted_style().add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows = albums.iter().enumerate().map(|(i, album)| {
        let num_str = format!("  {:02}", i + 1);
        let cells = vec![
            Cell::from(num_str).style(theme.muted_style()),
            Cell::from(album.title.clone()).style(theme.accent_style().add_modifier(Modifier::BOLD)),
            Cell::from(album.artist.clone()).style(theme.text_style()),
            Cell::from(album.year.clone()).style(theme.muted_style()),
        ];
        Row::new(cells).height(1)
    });

    let widths = [
        Constraint::Length(5),
        Constraint::Percentage(45),
        Constraint::Percentage(35),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .highlight_style(theme.selected_row_style())
        .highlight_symbol("▸ ");

    f.render_stateful_widget(table, area, state);
}

pub fn render_artist_list(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    title_text: &str,
    artists: &[ArtistSummary],
    state: &mut TableState,
    is_focused: bool,
) {
    let border_style = if is_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let title = Line::from(vec![
        Span::styled(format!(" Artistas: {} [2] ", title_text), theme.title_style()),
        Span::styled("── [Enter: Músicas | p: Tocar Todas | a: Add Fila | q: Esconder] ", theme.muted_style()),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title)
        .style(theme.default_block_style());

    let header_cells = ["#", "ARTISTA", "OUVINTES / DETALHES"]
        .iter()
        .map(|h| Cell::from(*h).style(theme.muted_style().add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows = artists.iter().enumerate().map(|(i, artist)| {
        let num_str = format!("  {:02}", i + 1);
        let cells = vec![
            Cell::from(num_str).style(theme.muted_style()),
            Cell::from(artist.name.clone()).style(theme.accent_style().add_modifier(Modifier::BOLD)),
            Cell::from(artist.subscribers_text.clone()).style(theme.text_style()),
        ];
        Row::new(cells).height(1)
    });

    let widths = [
        Constraint::Length(5),
        Constraint::Percentage(50),
        Constraint::Percentage(45),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .highlight_style(theme.selected_row_style())
        .highlight_symbol("▸ ");

    f.render_stateful_widget(table, area, state);
}
