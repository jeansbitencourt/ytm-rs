import QtQuick
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui

BarWidget {
  id: root

  moduleName: "jeansbitencourt.ytm-rs"

  property bool popupOpen: false
  property bool daemonRunning: false
  property bool tuiOpen: false
  property bool isPlaying: false
  property bool isPaused: false
  property bool isStopped: true
  property string trackTitle: ""
  property string trackArtist: ""
  property string trackAlbum: ""
  property int volume: 75
  property bool isLiked: false
  property double position: 0
  property double duration: 0

  readonly property string statusPath: (Quickshell.env("XDG_RUNTIME_DIR") || "/tmp") + "/ytm-rs/status.json"

  function sanitize(value, maxLen) {
    return String(value || "").replace(/[\r\n\t]+/g, " ").slice(0, maxLen)
  }

  function formatTime(secs) {
    var s = Math.max(0, Math.floor(Number(secs) || 0))
    var m = Math.floor(s / 60)
    var remS = s % 60
    return (m < 10 ? "0" : "") + m + ":" + (remS < 10 ? "0" : "") + remS
  }

  function applyStatus(raw) {
    try {
      if (typeof raw !== "string" || raw.length > 32768) return
      var data = JSON.parse(raw || "{}")
      root.daemonRunning = data.daemon_running === true || data.running === true
      root.tuiOpen = data.tui_open === true
      root.isPlaying = data.playing === true
      root.isPaused = data.paused === true
      root.isStopped = !root.isPlaying && !root.isPaused
      root.trackTitle = root.sanitize(data.title || "", 120)
      root.trackArtist = root.sanitize(data.artist || "", 100)
      root.trackAlbum = root.sanitize(data.album || "", 100)
      root.volume = typeof data.volume === "number" ? Math.max(0, Math.min(100, Math.round(data.volume))) : 75
      root.isLiked = data.liked === true
      root.position = Number(data.position || 0)
      root.duration = Number(data.duration || 0)
    } catch (e) {
      // Ignora erro transitório de JSON
    }
  }

  function runCli(args) {
    cliProcess.command = ["ytm-rs", "ctl"].concat(args)
    cliProcess.running = true
  }

  Process {
    id: cliProcess
    command: []
  }

  FileView {
    path: root.statusPath
    watchChanges: true
    atomicWrites: true
    printErrors: false
    onLoaded: root.applyStatus(text())
    onFileChanged: reload()
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.isPlaying ? "▶" : root.isPaused ? "⏸" : "🎵"
    active: root.isPlaying || root.tuiOpen

    tooltipText: root.isPlaying
      ? "Tocando: " + root.sanitize(root.trackArtist ? root.trackArtist + " - " + root.trackTitle : root.trackTitle, 140)
        + (root.isLiked ? " ♥" : "")
        + " · Vol: " + root.volume + "%\nBotão esquerdo: Focar janela | Botão direito: Menu"
      : root.isPaused
      ? "Pausado: " + root.sanitize(root.trackArtist ? root.trackArtist + " - " + root.trackTitle : root.trackTitle, 140)
        + " · Vol: " + root.volume + "%\nBotão esquerdo: Focar janela | Botão direito: Menu"
      : root.tuiOpen
      ? "YTM-RS: Janela aberta (Pronto)\nBotão esquerdo: Focar janela | Botão direito: Menu"
      : root.daemonRunning
      ? "YTM-RS: Segundo plano ativo\nBotão esquerdo: Abrir | Botão direito: Menu"
      : "YTM-RS (YouTube Music)\nBotão esquerdo: Abrir | Botão direito: Menu"

    onPressed: function(mouseButton) {
      if (!root.bar) return

      if (mouseButton === Qt.LeftButton) {
        // Foca na janela existente ou abre se não existir
        root.runCli(["focus"])
        return
      }

      if (mouseButton === Qt.RightButton) {
        // Abre modal com controles completos
        root.popupOpen = !root.popupOpen
        return
      }

      if (mouseButton === Qt.MiddleButton) {
        // Curtir música atual
        root.runCli(["like"])
        return
      }
    }

    onWheelMoved: function(delta) {
      if (delta > 0) {
        root.runCli(["vol-up"])
      } else if (delta < 0) {
        root.runCli(["vol-down"])
      }
    }
  }

  // Modal PopupCard com controles avançados ao clicar com o botão direito
  PopupCard {
    id: popup
    anchorItem: root
    bar: root.bar
    owner: root
    open: root.popupOpen
    contentWidth: popup.fittedContentWidth(Style.space(320))
    contentHeight: popup.fittedContentHeight(modalColumn.implicitHeight)

    Column {
      id: modalColumn
      anchors.fill: parent
      spacing: Style.space(10)

      // 1. Cabeçalho: Capa / Ícone + Título e Artista
      Row {
        width: parent.width
        spacing: Style.space(12)

        BorderSurface {
          width: Style.space(56)
          height: Style.space(56)
          radius: Style.spacing.labelGap
          color: Style.normalFillFor(root.bar ? root.bar.foreground : "#ffffff", Commons.Color.accent)
          borderSpec: Border.controlSpec("normal", root.bar ? root.bar.foreground : "#ffffff", Commons.Color.accent)

          Text {
            anchors.centerIn: parent
            text: root.isPlaying ? "▶" : root.isPaused ? "⏸" : "♫"
            color: Commons.Color.accent
            font.pixelSize: Style.font.displayMedium
            font.bold: true
          }
        }

        Column {
          width: parent.width - Style.space(68)
          spacing: Style.space(3)

          Text {
            textFormat: Text.PlainText
            text: root.trackTitle || (root.tuiOpen ? "YTM-RS: Pronto" : "Nenhuma música tocando")
            color: root.bar ? root.bar.foreground : "#ffffff"
            font.pixelSize: Style.font.subtitle
            font.bold: true
            elide: Text.ElideRight
            width: parent.width
          }

          Text {
            textFormat: Text.PlainText
            text: root.trackArtist || (root.tuiOpen ? "Janela do terminal aberta" : "YouTube Music TUI & Daemon")
            color: Qt.darker(root.bar ? root.bar.foreground : "#ffffff", 1.3)
            font.pixelSize: Style.font.bodySmall
            elide: Text.ElideRight
            width: parent.width
          }

          Text {
            textFormat: Text.PlainText
            text: root.isPlaying
              ? "● Reproduzindo (" + root.formatTime(root.position) + " / " + root.formatTime(root.duration) + ")"
              : root.isPaused
              ? "⏸ Pausado (" + root.formatTime(root.position) + " / " + root.formatTime(root.duration) + ")"
              : root.tuiOpen
              ? "● Janela ativa"
              : "■ Inativo"
            color: root.isPlaying ? Commons.Color.accent : root.isPaused ? "#F39C12" : "#888888"
            font.pixelSize: Style.font.caption
          }
        }
      }

      // 2. Barra de Progresso
      Rectangle {
        width: parent.width
        height: Style.space(4)
        radius: 2
        color: "#222a30"

        Rectangle {
          width: root.duration > 0 ? Math.min(parent.width, parent.width * (root.position / root.duration)) : 0
          height: parent.height
          radius: 2
          color: Commons.Color.accent
        }
      }

      // 3. Botões de Controle de Reprodução
      Row {
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: Style.space(8)

        Button {
          text: "⏮"
          onClicked: root.runCli(["prev"])
        }

        Button {
          text: root.isPlaying ? "⏸" : "▶"
          onClicked: root.runCli(["toggle"])
        }

        Button {
          text: "⏭"
          onClicked: root.runCli(["next"])
        }

        Button {
          text: root.isLiked ? "♥" : "♡"
          onClicked: root.runCli(["like"])
        }
      }

      // 4. Controle de Volume
      Row {
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: Style.space(10)

        Button {
          text: "🔉 -"
          onClicked: root.runCli(["vol-down"])
        }

        Text {
          anchors.verticalCenter: parent.verticalCenter
          text: "Volume: " + root.volume + "%"
          color: root.bar ? root.bar.foreground : "#ffffff"
          font.pixelSize: Style.font.bodySmall
        }

        Button {
          text: "🔊 +"
          onClicked: root.runCli(["vol-up"])
        }
      }

      // 5. Botões de Ação da Janela
      Row {
        width: parent.width
        spacing: Style.space(8)

        Button {
          width: (parent.width - Style.space(8)) / 2
          text: "Focar Janela"
          onClicked: {
            root.popupOpen = false
            root.runCli(["focus"])
          }
        }

        Button {
          width: (parent.width - Style.space(8)) / 2
          text: "Parar"
          onClicked: {
            root.runCli(["stop"])
          }
        }
      }
    }
  }
}
