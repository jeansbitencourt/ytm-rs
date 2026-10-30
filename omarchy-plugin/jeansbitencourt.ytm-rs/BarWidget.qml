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
  function close() { popupOpen = false }

  property bool daemonRunning: false
  property bool tuiOpen: false
  property bool isPlaying: false
  property bool isPaused: false
  property bool isStopped: true
  property string trackTitle: ""
  property string trackArtist: ""
  property string trackAlbum: ""
  property string coverUrl: ""
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
      root.coverUrl = root.sanitize(data.cover_url || "", 500)
      root.volume = typeof data.volume === "number" ? Math.max(0, Math.min(100, Math.round(data.volume))) : 75
      root.isLiked = data.liked === true
      root.position = Number(data.position || 0)
      root.duration = Number(data.duration || 0)
    } catch (e) {
      // Ignora erro transitório durante escrita atômica
    }
  }

  function runCommand(cmd) {
    if (root.bar && typeof root.bar.run === "function") {
      root.bar.run(cmd)
    } else {
      Util.execDetached(cmd)
    }
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

    onPressed: function(button) {
      if (button === Qt.LeftButton) {
        // Foca na janela existente ou abre uma nova
        root.runCommand("ytm-rs ctl focus")
        return
      }

      if (button === Qt.RightButton) {
        // Abre / fecha modal de controles
        root.popupOpen = !root.popupOpen
        return
      }

      if (button === Qt.MiddleButton) {
        // Curtir música atual
        root.runCommand("ytm-rs ctl like")
        return
      }
    }

    onWheelMoved: function(delta) {
      if (delta > 0) {
        root.runCommand("ytm-rs ctl vol-up")
      } else if (delta < 0) {
        root.runCommand("ytm-rs ctl vol-down")
      }
    }
  }

  // Modal PopupCard com controles avançados ao clicar com o botão direito
  PopupCard {
    id: popup
    anchorItem: button
    bar: root.bar
    owner: root
    open: root.popupOpen
    contentWidth: popup.fittedContentWidth(Style.space(320))
    contentHeight: popup.fittedContentHeight(modalColumn.implicitHeight)

    Column {
      id: modalColumn
      width: parent.width
      spacing: Style.space(10)

      // 1. Cabeçalho: Ícone + Título e Artista
      Row {
        width: parent.width
        spacing: Style.space(12)

        BorderSurface {
          width: Style.space(52)
          height: Style.space(52)
          radius: Style.spacing.labelGap
          color: Style.normalFillFor(root.bar ? root.bar.foreground : "#ffffff", Commons.Color.accent)
          borderSpec: Border.controlSpec("normal", root.bar ? root.bar.foreground : "#ffffff", Commons.Color.accent)
          clip: true

          Image {
            id: coverImage
            anchors.fill: parent
            source: root.coverUrl
            fillMode: Image.PreserveAspectCrop
            visible: status === Image.Ready && root.coverUrl !== ""
            asynchronous: true
            cache: true
            mipmap: true
            smooth: true
          }

          Text {
            anchors.centerIn: parent
            text: root.isPlaying ? "▶" : root.isPaused ? "⏸" : "♫"
            color: Commons.Color.accent
            font.pixelSize: Style.font.displayMedium
            font.bold: true
            visible: !coverImage.visible
          }
        }

        Column {
          width: parent.width - Style.space(64)
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
        spacing: Style.space(12)

        Button {
          text: "⏮"
          onClicked: root.runCommand("ytm-rs ctl prev")
        }

        Button {
          text: root.isPlaying ? "⏸ Pausar" : "▶ Tocar"
          onClicked: root.runCommand("ytm-rs ctl toggle")
        }

        Button {
          text: "⏭"
          onClicked: root.runCommand("ytm-rs ctl next")
        }

        Button {
          text: root.isLiked ? "♥" : "♡"
          onClicked: root.runCommand("ytm-rs ctl like")
        }
      }

      // 4. Controle de Volume
      Row {
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: Style.space(12)

        Button {
          text: "🔉 -"
          onClicked: root.runCommand("ytm-rs ctl vol-down")
        }

        Text {
          anchors.verticalCenter: parent.verticalCenter
          text: "Volume: " + root.volume + "%"
          color: root.bar ? root.bar.foreground : "#ffffff"
          font.pixelSize: Style.font.bodySmall
        }

        Button {
          text: "🔊 +"
          onClicked: root.runCommand("ytm-rs ctl vol-up")
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
            root.runCommand("ytm-rs ctl focus")
          }
        }

        Button {
          width: (parent.width - Style.space(8)) / 2
          text: "Parar"
          onClicked: root.runCommand("ytm-rs ctl stop")
        }
      }
    }
  }
}
