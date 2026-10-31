use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;

pub fn render_help_modal(f: &mut Frame, area: Rect, app: &App) {
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

pub fn render_auth_modal(f: &mut Frame, area: Rect, app: &App) {
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
        if let Some(prof) = &app.user_profile {
            Line::from(vec![
                Span::styled("Status da Conta: ", theme.title_style()),
                Span::styled("✓ Conectado ao YouTube Music ", theme.playing_indicator_style().add_modifier(Modifier::BOLD)),
                Span::styled(format!("── {} ({})", prof.name, prof.handle), theme.text_style()),
            ])
        } else if app.is_syncing_account {
            Line::from(vec![
                Span::styled("Status da Conta: ", theme.title_style()),
                Span::styled("⏳ Sincronizando dados com o Google...", theme.accent_style().add_modifier(Modifier::BOLD)),
            ])
        } else if app.account_sync_error.is_some() {
            Line::from(vec![
                Span::styled("Status da Conta: ", theme.title_style()),
                Span::styled("⚠️ Sessão / Cookie Expirado ", theme.liked_style().add_modifier(Modifier::BOLD)),
                Span::styled("(O Google rejeitou o cookie salvo)", theme.muted_style()),
            ])
        } else {
            Line::from(vec![
                Span::styled("Status da Conta: ", theme.title_style()),
                Span::styled("✓ Conectado ao YouTube Music", theme.playing_indicator_style().add_modifier(Modifier::BOLD)),
            ])
        }
    } else {
        Line::from(vec![
            Span::styled("Status da Conta: ", theme.title_style()),
            Span::styled("● Modo Público / Anônimo", theme.accent_style().add_modifier(Modifier::BOLD)),
            Span::styled(" (Busca, Rádio e reprodução liberados)", theme.muted_style()),
        ])
    };

    let p_status = Paragraph::new(vec![
        status_line,
        Line::from(Span::styled("A autenticação sincroniza suas Músicas Curtidas, Playlists e Perfil com sua conta Google.", theme.text_style())),
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
