use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use ratatui_image::StatefulImage;

use crate::app::{ActivePanel, ContentView, App};
use crate::innertube::SearchFilter;

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

    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(main_chunks[0]);
    app.header_search_area = header_chunks[0];

    // 1. Header (Search Box on Left + Daemon Status on Right)
    render_header(f, header_chunks[0], header_chunks[1], app);

    // 2. Middle Content (Sidebar 28% + Main View 72%)
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
        .split(main_chunks[1]);

    // Split left side into Library (top) and Album Cover (bottom)
    let sidebar_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(6)])
        .split(content_chunks[0]);

    let counts = (
        app.liked_tracks.len(),
        app.history_tracks.len(),
        app.player_state.queue.len(),
    );

    let is_sidebar_focused = app.active_panel == ActivePanel::Sidebar;
    sidebar::render_sidebar(
        f,
        sidebar_chunks[0],
        theme,
        &mut app.sidebar_state,
        is_sidebar_focused,
        counts,
    );

    render_cover_art(
        f,
        sidebar_chunks[1],
        theme,
        &mut app.cover_protocol,
        app.player_state.current_track.is_some(),
    );

    let is_tracklist_focused = app.active_panel == ActivePanel::Tracklist;
    let current_vid = app.player_state.current_track.as_ref().map(|t| t.video_id.as_str());
    let is_playing = app.player_state.status == crate::audio::PlaybackStatus::Playing;
    app.tracklist_area = content_chunks[1];

    match app.content_view {
        ContentView::Tracks => {
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
                &mut app.thumb_protocols,
            );
        }
        ContentView::Albums => {
            tracklist::render_album_list(
                f,
                content_chunks[1],
                theme,
                &app.active_category_title,
                &app.search_results_albums,
                &mut app.album_table_state,
                is_tracklist_focused,
                &mut app.thumb_protocols,
            );
        }
        ContentView::Artists => {
            tracklist::render_artist_list(
                f,
                content_chunks[1],
                theme,
                &app.active_category_title,
                &app.search_results_artists,
                &mut app.artist_table_state,
                is_tracklist_focused,
                &mut app.thumb_protocols,
            );
        }
    }

    // 3. Player Bar
    playerbar::render_playerbar(f, main_chunks[2], theme, &app.player_state);

    // 4. Help Modal if toggled
    if app.show_help {
        render_help_modal(f, size, app);
    }

    // 5. Auth Modal if toggled
    if app.show_auth_modal {
        render_auth_modal(f, size, app);
    }
}

fn render_cover_art(
    f: &mut Frame,
    area: Rect,
    theme: &crate::theme::Theme,
    cover_protocol: &mut Option<Box<dyn ratatui_image::protocol::StatefulProtocol>>,
    is_playing: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.unfocused_border_style())
        .title(Span::styled(" ♫ Capa em Reprodução ", theme.title_style()))
        .style(theme.default_block_style());

    let inner_area = block.inner(area);
    f.render_widget(block, area);

    if inner_area.width < 4 || inner_area.height < 2 {
        return;
    }

    if let Some(protocol) = cover_protocol {
        let image_widget = StatefulImage::new(None);
        f.render_stateful_widget(image_widget, inner_area, protocol);
    } else {
        let placeholder = if is_playing {
            vec![
                Line::from(""),
                Line::from(Span::styled("⏳ Carregando capa...", theme.muted_style())),
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

pub fn get_search_filter_button_at(col: u16, row: u16, search_area: Rect) -> Option<SearchFilter> {
    if row > search_area.y + 1 {
        return None;
    }
    let start_x = search_area.x + 1;
    if col < start_x + 27 {
        return None;
    }
    let rel_x = col - start_x;
    if (27..=40).contains(&rel_x) {
        Some(SearchFilter::Tracks)
    } else if (41..=52).contains(&rel_x) {
        Some(SearchFilter::Albums)
    } else if (53..=67).contains(&rel_x) {
        Some(SearchFilter::Artists)
    } else {
        None
    }
}

fn render_header(f: &mut Frame, search_area: Rect, status_area: Rect, app: &App) {
    let theme = &app.theme;

    let is_search_focused = app.active_panel == ActivePanel::Search;
    let search_border_style = if is_search_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let chip = |filter: SearchFilter| {
        let is_active = app.search_filter == filter;
        if is_active {
            Span::styled(format!("[{}]", filter.label()), theme.selected_row_style())
        } else {
            Span::styled(format!("[{}]", filter.label()), theme.muted_style())
        }
    };

    let title_line = Line::from(vec![
        Span::styled(" Busca [/] ── Filtro [Tab]: ", theme.title_style()),
        chip(SearchFilter::Tracks),
        Span::raw(" "),
        chip(SearchFilter::Albums),
        Span::raw(" "),
        chip(SearchFilter::Artists),
        Span::raw(" "),
    ]);

    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(search_border_style)
        .title(title_line)
        .style(theme.default_block_style());

    let search_text = if app.search_input.is_empty() {
        if is_search_focused {
            Span::styled("█", theme.accent_style())
        } else {
            Span::styled("digite para buscar músicas, álbuns ou artistas...", theme.muted_style())
        }
    } else {
        let cursor = if is_search_focused { "█" } else { "" };
        Span::styled(format!("{}{}", app.search_input, cursor), theme.text_style())
    };

    let search_line = Line::from(vec![
        Span::styled("🔍 ", theme.accent_style().add_modifier(Modifier::BOLD)),
        search_text,
    ]);

    let search_p = Paragraph::new(search_line).block(search_block);
    f.render_widget(search_p, search_area);

    // Right: Status indicator with Account info
    let status_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.unfocused_border_style())
        .style(theme.default_block_style());

    let auth_span = if app.auth.is_authenticated() {
        Span::styled("👤 Conectada", theme.playing_indicator_style())
    } else {
        Span::styled("👤 Modo Público [c]", theme.accent_style())
    };

    let status_line = Line::from(vec![
        auth_span,
        Span::styled(" | ● Daemon: ", theme.muted_style()),
        Span::styled("Ativo", theme.playing_indicator_style()),
        Span::styled(" | MPRIS2: ", theme.muted_style()),
        Span::styled("OK", theme.accent_style()),
        Span::styled(" | ?: Ajuda", theme.muted_style()),
    ]);

    let status_p = Paragraph::new(status_line)
        .block(status_block)
        .alignment(Alignment::Right);
    f.render_widget(status_p, status_area);
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
        ("Navegação & Busca:", ""),
        ("  h, j, k, l / Setas", "Navegar entre painéis e rolar listas"),
        ("  1 / 2 / /", "Focar Biblioteca [1], Painel Principal [2] ou Busca [/]"),
        ("  Tab / Shift+Tab", "Alternar filtro de busca (Músicas / Álbuns / Artistas)"),
        ("  Enter", "Reproduzir faixa ou abrir álbum / artista"),
        ("  Backspace / Esc", "Voltar à lista anterior / Sair da busca"),
        ("", ""),
        ("Controles de Reprodução:", ""),
        ("  Space", "Alternar Reproduzir / Pausar (Play / Pause)"),
        ("  p", "Tocar álbum/artista completo de uma vez (ou faixa anterior)"),
        ("  n / p (< / >)", "Próxima faixa / Faixa anterior"),
        ("  + / -", "Aumentar / Diminuir volume (±5%)"),
        ("  f", "Favoritar música atual (Like)"),
        ("  a", "Adicionar item selecionado (faixa/álbum/artista) à fila"),
        ("  r", "Iniciar Rádio Infinita a partir da faixa selecionada"),
        ("", ""),
        ("Controles de Janela & Sistema:", ""),
        ("  c / C", "Gerenciar Conta / Autenticação (Cookie YTM)"),
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

fn render_auth_modal(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;

    let width = 76.min(area.width.saturating_sub(4));
    let height = 20.min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.focused_border_style())
        .title(Span::styled(" Autenticação YouTube Music (Conta Google) ", theme.title_style()))
        .style(theme.default_block_style());

    let inner_area = block.inner(modal_area);
    f.render_widget(block, modal_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Status & Info
            Constraint::Length(5), // Instructions
            Constraint::Length(3), // Input box
            Constraint::Length(2), // Feedback / Status Message
            Constraint::Length(2), // Shortcut footer
        ])
        .split(inner_area);

    // 1. Status Info
    let status_line = if app.auth.is_authenticated() {
        Line::from(vec![
            Span::styled("Status da Conta: ", theme.title_style()),
            Span::styled("✓ Conectado ao YouTube Music", theme.playing_indicator_style().add_modifier(Modifier::BOLD)),
            Span::styled(" (Sessão SAPISID ativa)", theme.muted_style()),
        ])
    } else {
        Line::from(vec![
            Span::styled("Status da Conta: ", theme.title_style()),
            Span::styled("● Modo Público / Anônimo", theme.accent_style().add_modifier(Modifier::BOLD)),
            Span::styled(" (Busca, Rádio e reprodução liberados)", theme.muted_style()),
        ])
    };

    let p_status = Paragraph::new(vec![
        status_line,
        Line::from(Span::styled("A autenticação sincroniza suas Músicas Curtidas e Playlists com sua conta Google.", theme.text_style())),
    ]);
    f.render_widget(p_status, chunks[0]);

    // 2. Instructions
    let instr_lines = vec![
        Line::from(Span::styled("Como obter seu Cookie:", theme.title_style())),
        Line::from(Span::styled(" 1. Acesse https://music.youtube.com no seu navegador e faça login", theme.muted_style())),
        Line::from(Span::styled(" 2. Abra o DevTools (F12) -> aba Rede (Network) -> recarregue a página", theme.muted_style())),
        Line::from(Span::styled(" 3. Clique em qualquer requisição e copie o valor do cabeçalho 'cookie'", theme.muted_style())),
        Line::from(Span::styled(" 4. Cole o valor abaixo (Ctrl+Shift+V) e pressione Enter", theme.muted_style())),
    ];
    let p_instr = Paragraph::new(instr_lines);
    f.render_widget(p_instr, chunks[1]);

    // 3. Input Box
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.focused_border_style())
        .title(Span::styled(" Cole o Cookie aqui ", theme.accent_style()));

    let display_text = if app.auth_input.is_empty() {
        Span::styled("Cole o cabeçalho cookie aqui e pressione Enter... █", theme.muted_style())
    } else {
        let masked = if app.auth_input.len() > 46 {
            format!("{}... ({} caracteres) █", &app.auth_input[..36], app.auth_input.len())
        } else {
            format!("{} █", app.auth_input)
        };
        Span::styled(masked, theme.text_style())
    };

    let p_input = Paragraph::new(display_text).block(input_block);
    f.render_widget(p_input, chunks[2]);

    // 4. Feedback Message
    if let Some((msg, is_success)) = &app.auth_message {
        let style = if *is_success {
            theme.playing_indicator_style()
        } else {
            theme.accent_style()
        };
        let p_msg = Paragraph::new(Span::styled(msg.as_str(), style));
        f.render_widget(p_msg, chunks[3]);
    }

    // 5. Footer Shortcuts
    let footer_line = Line::from(vec![
        Span::styled("[Enter] Salvar Cookie  ", theme.accent_style().add_modifier(Modifier::BOLD)),
        if app.auth.is_authenticated() {
            Span::styled("[l] Desconectar Conta  ", theme.liked_style())
        } else {
            Span::raw("")
        },
        Span::styled("[Esc] Fechar Janela", theme.muted_style()),
    ]);
    let p_footer = Paragraph::new(footer_line).alignment(Alignment::Right);
    f.render_widget(p_footer, chunks[4]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use crate::innertube::SearchFilter;

    #[test]
    fn test_search_filter_button_hit_detection() {
        let search_area = Rect::new(0, 0, 80, 3);

        // Outside row range
        assert_eq!(get_search_filter_button_at(30, 2, search_area), None);

        // Before first button
        assert_eq!(get_search_filter_button_at(10, 0, search_area), None);

        // Tracks button: offsets 27..=40 (col 28..=41 since start_x = 1)
        assert_eq!(get_search_filter_button_at(28, 0, search_area), Some(SearchFilter::Tracks));
        assert_eq!(get_search_filter_button_at(41, 0, search_area), Some(SearchFilter::Tracks));

        // Albums button: offsets 41..=52 (col 42..=53)
        assert_eq!(get_search_filter_button_at(42, 0, search_area), Some(SearchFilter::Albums));
        assert_eq!(get_search_filter_button_at(53, 0, search_area), Some(SearchFilter::Albums));

        // Artists button: offsets 53..=67 (col 54..=68)
        assert_eq!(get_search_filter_button_at(54, 0, search_area), Some(SearchFilter::Artists));
        assert_eq!(get_search_filter_button_at(68, 0, search_area), Some(SearchFilter::Artists));

        // Beyond artists button
        assert_eq!(get_search_filter_button_at(70, 0, search_area), None);
    }
}

