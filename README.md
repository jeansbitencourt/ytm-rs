# YTM-RS: Cliente de Terminal & Daemon para YouTube Music

![YTM-RS Icon](pkgbuild/ytm-rs.svg)

**YTM-RS** é um cliente de terminal (TUI) ultrarrápido e daemon de áudio para o **YouTube Music**, construído em **Rust + Ratatui + Tokio** e integrado profundamente ao ecossistema desktop **Omarchy** (Hyprland + Omarchy Shell / Quickshell).

O `ytm-rs` consome menos de **18 MB de RAM**, funciona **imediatamente sem login** e permite autenticação opcional da biblioteca. A reprodução de streams permanece anônima por padrão; uma fonte de cookies separada pode ser configurada deliberadamente para o extractor.

---

## Recursos Principais

- **Arquitetura Client-Daemon**: O processo de áudio em background (`ytm-rs daemon`) toca via socket UNIX e gerencia o `mpv` sem abrir uma janela durante a reprodução de áudio. Ao fechar a TUI (`q`), sua música continua tocando sem interrupção.
- **Top Bar do Omarchy (Quickshell & Waybar)**:
  - Servidor **MPRIS2 D-Bus** nativo (`org.mpris.MediaPlayer2.ytmrs`) integrado ao widget de áudio padrão do sistema.
  - Widget nativo para a Top Bar (`jeansbitencourt.ytm-rs`) com suporte a **Clique Esquerdo** (Play/Pause), **Clique do Meio** (Favoritar/Like), **Clique Direito** (Abrir/Esconder TUI no Scratchpad) e **Scroll** (Ajuste de Volume).
- **Interface Ratatui Estilo spotify-tui**: Layout modular em 4 blocos (Busca, Biblioteca, Faixas e Player Bar) com bordas arredondadas e suporte a temas ao vivo do Omarchy.
- **Sincronização de Tema Dinâmica**: Lê e monitora em tempo real as cores de `~/.local/state/omarchy/current/theme/colors.toml` no Omarchy. Fora do Omarchy, oferece suporte a temas customizados em `~/.config/ytm-rs/theme.toml` com fallback automático para Tokyo Night.
- **Detecção Inteligente de Terminal**: Compatível com múltiplos emuladores de terminal (**Foot**, **Kitty**, **Ghostty**, **Alacritty**, **WezTerm**, ou `$TERMINAL`). Renderiza capas de álbuns via **Sixel**, **Kitty Graphics Protocol** ou **Halfblocks** Unicode.
- **Transições Instantâneas (Prefetching)**: Worker assíncrono resolve a URL de áudio Opus 251 da próxima faixa assim que a música atual atinge 80%. Para reprodução anônima, o comportamento existente é mantido; com cookies de playback, o prefetch começa desativado e pode ser habilitado explicitamente.
- **Rádio Dinâmica Infinita**: Pressione `r` em qualquer faixa para gerar uma fila de rádio infinita baseada no algoritmo do YouTube Music (`RDAMVM{videoId}`).
- **Vídeo em Janela Nativa**: Pressione `v` para alternar entre áudio e vídeo da faixa atual. O programa aceita vídeos oficiais e vídeos enviados por usuários; candidatos classificados como faixa de áudio só são aceitos se houver um stream de vídeo. Posição, pausa e volume continuam no daemon; se o vídeo não puder abrir, o áudio é retomado.
- **Atalhos Orientados a Teclado (Vim-like)**.
- **Internacionalização**: a interface suporta Português (Brasil) e English por meio do catálogo centralizado do `rust-i18n`. Por padrão, acompanha o idioma escolhido no Omarchy (`~/.config/omarchy/language`) ou as variáveis de locale do Linux. Pressione `F2` na TUI para escolher `Seguir sistema`, `Português (Brasil)` ou `English`; a preferência fica em `~/.config/ytm-rs/config.toml` e a troca não interrompe a reprodução.

---

## Tabela de Atalhos de Teclado (TUI)

| Tecla / Gatilho | Ação | Descrição |
|---|---|---|
| `/` | **Buscar** | Foca na barra de pesquisa no topo |
| `1` | **Focar Biblioteca** | Alterna foco para o painel de bibliotecas/playlists à esquerda |
| `2` | **Focar Faixas** | Alterna foco para a tabela de músicas à direita |
| `h` `j` `k` `l` ou `Setas` | **Navegação Vim** | Rola listas e alterna entre painéis |
| `Enter` | **Reproduzir** | Toca a faixa selecionada ou entra na categoria da biblioteca |
| `Space` | **Play / Pause** | Alterna reprodução e pausa global |
| `n` / `p` | **Próxima / Anterior** | Avança ou retorna uma faixa na fila |
| `+` / `-` | **Volume** | Aumenta ou diminui o volume em 5% |
| `f` | **Favoritar (Like)** | Adiciona ou remove dos favoritos (com notificação desktop) |
| `a` | **Adicionar à Fila** | Enfileira a música selecionada para tocar a seguir |
| `r` | **Rádio Infinita** | Inicia rádio contínua baseada na faixa selecionada |
| `v` | **Vídeo** | Abre ou fecha a janela nativa de vídeo para a faixa atual, quando disponível |
| `?` | **Ajuda** | Abre o menu com a lista de todos os atalhos |
| `q` | **Esconder TUI** | Fecha a janela mantendo o áudio tocando em background |
| `Q` ou `Ctrl+Q` | **Sair Total** | Encerra a janela TUI e o daemon de segundo plano |

---

## Controle pela Top Bar e CLI (`ytm-rs ctl`)

Subcomandos remotos ultrarrápidos respondem em **< 2ms** através do socket `$XDG_RUNTIME_DIR/ytm-rs/state.sock` (ou do diretório temporário do usuário quando `XDG_RUNTIME_DIR` não está definido):

```bash
# Alternar reprodução
ytm-rs ctl toggle

# Próxima / Anterior
ytm-rs ctl next
ytm-rs ctl prev

# Ajustar volume
ytm-rs ctl vol-up
ytm-rs ctl vol-down
ytm-rs ctl volume 80

# Curtir música atual
ytm-rs ctl like

# Obter estado ou JSON para barra
ytm-rs ctl status
ytm-rs ctl waybar
```

O estado distingue `buffering`, `playing`, `paused` e `stopped`. Falhas ficam no campo `error_message` de `ctl status` e no log `${XDG_RUNTIME_DIR}/ytm-rs/daemon.log`.

---

## Autenticação da Biblioteca e da Reprodução (`ytm-rs auth`)

O aplicativo funciona sem login para buscas, reprodução e rádios. OAuth e cookies salvos com `auth login`/`auth cookies` são usados para acessar a biblioteca e sincronizar curtidas; eles **não** são enviados ao extractor de streams.

```bash
# Ver o status separado da biblioteca e da reprodução
ytm-rs auth status

# Autenticar a biblioteca (cookies no formato de cabeçalho HTTP)
ytm-rs auth login
cat cookie-header.txt | ytm-rs auth cookies
```

O comando `auth cookies` lê da entrada padrão para evitar expor o valor do cookie nos argumentos do processo. Esse cabeçalho é apenas para chamadas da biblioteca; ele não é convertido em arquivo Netscape nem enviado ao `yt-dlp`.

O playback usa `yt-dlp` e continua anônimo por padrão. Para experimentar extração autenticada, selecione explicitamente uma fonte:

```bash
# Deixar o yt-dlp ler um perfil do navegador
ytm-rs auth playback browser firefox --profile default

# Ou importar um arquivo Netscape existente. O aplicativo faz uma cópia privada (0600).
ytm-rs auth playback cookies /caminho/para/cookies.txt

# Ver configuração, desabilitar ou habilitar prefetch autenticado
ytm-rs auth playback status
ytm-rs auth playback inspect dQw4w9WgXcQ
ytm-rs auth playback prefetch on
ytm-rs auth playback prefetch off
ytm-rs auth playback disable

# Desconectar a biblioteca e apagar a configuração/cópia de cookies de playback
ytm-rs auth logout
```

O playback autenticado é **experimental** e fica desativado por padrão. A TUI apenas exibe o estado; a configuração é feita pela CLI. O login OAuth da biblioteca não é uma credencial suportada pelo extractor do YouTube do `yt-dlp`. O uso de uma conta no `yt-dlp` pode levar a bloqueio temporário ou permanente. Não há garantia de que o YouTube Music aplique benefícios Premium a streams resolvidos por terceiros.

Na avaliação feita em 2026-11-16 com uma conta Premium, duas consultas autenticadas à faixa pública usada na linha de base retornaram apenas storyboards, sem formatos de áudio. A consulta anônima havia listado Opus perto de 129 kb/s e AAC perto de 130 kb/s. Portanto, **nenhum benefício concreto de Premium foi demonstrado para a reprodução do daemon**; isso é um resultado limitado à faixa e ao caminho `yt-dlp` testados, não uma afirmação sobre os aplicativos oficiais ou todo o catálogo. A interface reporta a fonte como **configurada**, sem afirmar que foi validada ou que Premium está ativo. Em modo autenticado, uma falha não cai silenciosamente para extração anônima, os retries automáticos ficam desativados e o prefetch fica desativado por padrão. `auth logout` limpa também a fonte de playback e o arquivo Netscape importado.

`auth playback inspect <video-id>` lista metadados de formatos sem baixar mídia, salvar URLs ou imprimir headers. Se uma consulta autenticada retornar um formato de áudio que justifique reprodução, aguarde ao menos uma hora antes de qualquer teste curto; não habilite prefetch para essa avaliação.

No Arch, o pacote `yt-dlp` fornece `yt-dlp-ejs`; o runtime JavaScript `deno` é recomendado para os desafios do YouTube. Para Chromium com chaveiro GNOME, `python-secretstorage` é opcional e necessário para o acesso ao chaveiro. Essas dependências são opcionais no pacote do YTM-RS porque a reprodução anônima continua disponível sem elas.

---

## Compilação e Instalação

```bash
# 1. Compilar binário release
./bin/build

# 2. Executar suíte de testes
./bin/test

# 3. Instalar no sistema (Desktop + Ícone + Plugin Top Bar)
./bin/install
```

---

## Licença

Distribuído sob a licença [MIT](LICENSE). Copyright (c) 2026 Jean Bitencourt.
