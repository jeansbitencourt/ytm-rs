use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::{ActivePanel, App};

pub mod playerbar;
pub mod sidebar;
pub mod tracklist;

pub fn render_ui(f: &mut Frame, app: &mut App) {
    let size = f.area();
    let theme = &app.theme;

    // Outer container layout: Header (3) | Content (Min 8) | Player (5)
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(5),
        ])
        .split(size);

    app.playerbar_area = main_chunks[2];

    // 1. Header (Search Box on Left + Daemon Status on Right)
    render_header(f, main_chunks[0], app);

    // 2. Middle Content (Sidebar 28% + Tracklist 72%)
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
        .split(main_chunks[1]);

    let counts = (
        app.liked_tracks.len(),
        app.history_tracks.len(),
        app.player_state.queue.len(),
    );

    let is_sidebar_focused = app.active_panel == ActivePanel::Sidebar;
    sidebar::render_sidebar(
        f,
        content_chunks[0],
        theme,
        &mut app.sidebar_state,
        is_sidebar_focused,
        counts,
    );

    let is_tracklist_focused = app.active_panel == ActivePanel::Tracklist;
    let current_vid = app.player_state.current_track.as_ref().map(|t| t.video_id.as_str());
    let is_playing = app.player_state.status == crate::audio::PlaybackStatus::Playing;

    tracklist::render_tracklist(
        f,
        content_chunks[1],
        theme,
        &app.active_category_title,
        &app.displayed_tracks,
        current_vid,
        is_playing,
        &mut app.tracklist_state,
        is_tracklist_focused,
    );

    // 3. Player Bar
    playerbar::render_playerbar(f, main_chunks[2], theme, &app.player_state);

    // 4. Help Modal if toggled
    if app.show_help {
        render_help_modal(f, size, app);
    }
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let is_search_focused = app.active_panel == ActivePanel::Search;
    let search_border_style = if is_search_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(search_border_style)
        .style(theme.default_block_style());

    let search_text = if app.search_input.is_empty() {
        if is_search_focused {
            Span::styled("█", theme.accent_style())
        } else {
            Span::styled("pressione '/' para buscar músicas, artistas ou álbuns...", theme.muted_style())
        }
    } else {
        let cursor = if is_search_focused { "█" } else { "" };
        Span::styled(format!("{}{}", app.search_input, cursor), theme.text_style())
    };

    let search_line = Line::from(vec![
        Span::styled("🔍 Buscar [/]: ", theme.accent_style().add_modifier(Modifier::BOLD)),
        search_text,
    ]);

    let search_p = Paragraph::new(search_line).block(search_block);
    f.render_widget(search_p, chunks[0]);

    // Right: Status indicator
    let status_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.unfocused_border_style())
        .style(theme.default_block_style());

    let status_line = Line::from(vec![
        Span::styled("● Rust Daemon: ", theme.muted_style()),
        Span::styled("Ativo", theme.playing_indicator_style()),
        Span::styled(" | MPRIS2: ", theme.muted_style()),
        Span::styled("Conectado", theme.accent_style()),
        Span::styled(" | ?: Ajuda", theme.muted_style()),
    ]);

    let status_p = Paragraph::new(status_line)
        .block(status_block)
        .alignment(Alignment::Right);
    f.render_widget(status_p, chunks[1]);
}

fn render_help_modal(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;

    // Centered modal calculation
    let width = 74.min(area.width.saturating_sub(4));
    let height = 24.min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.focused_border_style())
        .title(Span::styled(" Mapa de Atalhos e Teclado (YTM-RS) ", theme.title_style()))
        .style(theme.default_block_style());

    let shortcuts = [
        ("Navegação:", ""),
        ("  h, j, k, l / Setas", "Navegar entre painéis e rolar faixas"),
        ("  1 / 2 / /", "Focar Biblioteca [1], Faixas [2] ou Busca [/]"),
        ("  Enter", "Reproduzir faixa ou carregar item selecionado"),
        ("  Esc", "Sair do modo de busca / fechar este modal"),
        ("", ""),
        ("Controles de Reprodução:", ""),
        ("  Space", "Alternar Reproduzir / Pausar (Play / Pause)"),
        ("  n / p", "Próxima faixa / Faixa anterior"),
        ("  + / -", "Aumentar / Diminuir volume (±5%)"),
        ("  f", "Favoritar música atual (Like)"),
        ("  a", "Adicionar faixa selecionada à fila de reprodução"),
        ("  r", "Iniciar Rádio Infinita a partir da faixa selecionada"),
        ("", ""),
        ("Controles de Janela & Sistema:", ""),
        ("  q", "Esconder janela TUI (continua tocando em segundo plano)"),
        ("  Q / Ctrl+Q", "Encerrar aplicativo e daemon de áudio"),
        ("  ?", "Abrir / Fechar este menu de ajuda"),
    ];

    let lines: Vec<Line> = shortcuts
        .iter()
        .map(|(key, desc)| {
            if desc.is_empty() {
                Line::from(Span::styled(*key, theme.title_style()))
            } else {
                Line::from(vec![
                    Span::styled(format!("{:<24}", key), theme.accent_style().add_modifier(Modifier::BOLD)),
                    Span::styled(*desc, theme.text_style()),
                ])
            }
        })
        .collect();

    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, modal_area);
}
