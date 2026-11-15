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
  property bool isBuffering: false
  property bool isStopped: true
  property string playbackError: ""
  property string trackTitle: ""
  property string trackArtist: ""
  property string trackAlbum: ""
  property string coverUrl: ""
  property int volume: 75
  property bool isLiked: false
  property double position: 0
  property double duration: 0
  property double lastStatusUpdatedAt: 0
  property var widgetText: ({})
  property string statusTooltip: ""

  function label(key, replacements) {
    var text = String(root.widgetText[key] || "")
    if (replacements) {
      for (var name in replacements)
        text = text.split("{" + name + "}").join(String(replacements[name]))
    }
    return text
  }

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

  function markDaemonOffline() {
    root.daemonRunning = false
    root.tuiOpen = false
    root.isPlaying = false
    root.isPaused = false
    root.isBuffering = false
    root.isStopped = true
    root.playbackError = ""
    root.trackTitle = ""
    root.trackArtist = ""
    root.trackAlbum = ""
    root.coverUrl = ""
    root.position = 0
    root.duration = 0
    root.statusTooltip = root.label("offline_tooltip")
  }

  function applyStatus(raw) {
    try {
      if (typeof raw !== "string" || raw.length > 32768) return
      var data = JSON.parse(raw || "{}")
      root.daemonRunning = data.daemon_running === true || data.running === true
      root.tuiOpen = data.tui_open === true
      root.isPlaying = data.playing === true
      root.isPaused = data.paused === true
      root.isBuffering = data.buffering === true || data.status === "buffering"
      root.isStopped = data.stopped === true || data.status === "stopped"
      if (data.status === undefined) root.isStopped = !root.isPlaying && !root.isPaused
      root.playbackError = root.sanitize(data.error || "", 180)
      root.trackTitle = root.sanitize(data.title || "", 120)
      root.trackArtist = root.sanitize(data.artist || "", 100)
      root.trackAlbum = root.sanitize(data.album || "", 100)
      root.coverUrl = root.sanitize(data.cover_url || "", 500)
      root.volume = typeof data.volume === "number" ? Math.max(0, Math.min(100, Math.round(data.volume))) : 75
      root.isLiked = data.liked === true
      root.position = Number(data.position || 0)
      root.duration = Number(data.duration || 0)
      root.lastStatusUpdatedAt = Number(data.updated_at_ms || 0)
      root.widgetText = data.widget || ({})
      root.statusTooltip = String(data.tooltip || "").replace(/[\r\t]+/g, " ").slice(0, 1000)
    } catch (e) {
      // Ignore transient errors while the file is being written atomically
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
    id: statusFile
    path: root.statusPath
    watchChanges: true
    atomicWrites: true
    printErrors: false
    onLoaded: {
      root.applyStatus(text())
      statusRetry.stop()
    }
    onLoadFailed: statusRetry.start()
    onFileChanged: reload()
  }

  // Retry only while the file cannot be loaded.
  Timer {
    id: statusRetry
    interval: 1000
    running: false
    repeat: true
    onTriggered: statusFile.reload()
  }

  Timer {
    interval: 2000
    running: true
    repeat: true
    onTriggered: {
      if (root.lastStatusUpdatedAt > 0 && Date.now() - root.lastStatusUpdatedAt > 6000)
        root.markDaemonOffline()
    }
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.isPlaying ? "▶" : root.isPaused ? "⏸" : root.isBuffering ? "⏳" : root.playbackError ? "⚠" : "🎵"
    active: root.isPlaying || root.isPaused || root.isBuffering || root.tuiOpen

    tooltipText: root.statusTooltip

    onPressed: function(button) {
      if (button === Qt.LeftButton) {
        // Focus the existing window or open a new one
        root.runCommand("ytm-rs ctl focus")
        return
      }

      if (button === Qt.RightButton) {
        // Open / close the control modal
        root.popupOpen = !root.popupOpen
        return
      }

      if (button === Qt.MiddleButton) {
        // Like the current track
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

  // PopupCard modal with advanced controls on right-click
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

      // 1. Header: icon + title and artist
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
            text: root.isPlaying ? "▶" : root.isPaused ? "⏸" : root.isBuffering ? "⏳" : root.playbackError ? "⚠" : "♫"
            color: root.playbackError ? "#E74C3C" : Commons.Color.accent
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
            text: root.trackTitle || (root.tuiOpen ? root.label("ready") : root.label("no_track"))
            color: root.bar ? root.bar.foreground : "#ffffff"
            font.pixelSize: Style.font.subtitle
            font.bold: true
            elide: Text.ElideRight
            width: parent.width
          }

          Text {
            textFormat: Text.PlainText
            text: root.trackArtist || (root.tuiOpen ? root.label("terminal_open") : root.label("description"))
            color: Qt.darker(root.bar ? root.bar.foreground : "#ffffff", 1.3)
            font.pixelSize: Style.font.bodySmall
            elide: Text.ElideRight
            width: parent.width
          }

          Text {
            textFormat: Text.PlainText
            text: root.isPlaying
              ? root.label("playing_progress", { position: root.formatTime(root.position), duration: root.formatTime(root.duration) })
              : root.isPaused
              ? root.label("paused_progress", { position: root.formatTime(root.position), duration: root.formatTime(root.duration) })
              : root.isBuffering
              ? root.label("buffering_progress")
              : root.playbackError
              ? root.label("failed_progress")
              : root.tuiOpen
              ? root.label("window_active")
              : root.label("idle")
            color: root.isPlaying || root.isBuffering ? Commons.Color.accent : root.isPaused ? "#F39C12" : root.playbackError ? "#E74C3C" : "#888888"
            font.pixelSize: Style.font.caption
          }
        }
      }

      // 2. Progress bar
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

      // 3. Playback control buttons
      Row {
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: Style.space(12)

        Button {
          text: "⏮"
          onClicked: root.runCommand("ytm-rs ctl prev")
        }

        Button {
          text: root.isBuffering ? root.label("buffering_button") : root.isPlaying ? root.label("pause_button") : root.label("play_button")
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

      // 4. Volume control
      Row {
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: Style.space(12)

        Button {
          text: "🔉 -"
          onClicked: root.runCommand("ytm-rs ctl vol-down")
        }

        Text {
          anchors.verticalCenter: parent.verticalCenter
          text: root.label("volume") + root.volume + "%"
          color: root.bar ? root.bar.foreground : "#ffffff"
          font.pixelSize: Style.font.bodySmall
        }

        Button {
          text: "🔊 +"
          onClicked: root.runCommand("ytm-rs ctl vol-up")
        }
      }

      // 5. Window action buttons
      Row {
        width: parent.width
        spacing: Style.space(8)

        Button {
          width: (parent.width - Style.space(8)) / 2
          text: root.label("focus_button")
          onClicked: {
            root.popupOpen = false
            root.runCommand("ytm-rs ctl focus")
          }
        }

        Button {
          width: (parent.width - Style.space(8)) / 2
          text: root.label("stop_button")
          onClicked: root.runCommand("ytm-rs ctl stop")
        }
      }
    }
  }
}
