import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

BarWidget {
  id: root

  moduleName: "jeansbitencourt.ytm-rs"

  property bool isRunning: false
  property bool isPaused: false
  property bool isStopped: true
  property string trackTitle: ""
  property string trackArtist: ""
  property int volume: 75
  property bool isLiked: false
  property double position: 0
  property double duration: 0

  readonly property string statusPath: (Quickshell.env("XDG_RUNTIME_DIR") || "/tmp") + "/ytm-rs/status.json"

  function sanitize(value, maxLen) {
    return String(value || "").replace(/[\r\n\t]+/g, " ").slice(0, maxLen)
  }

  function applyStatus(raw) {
    try {
      if (typeof raw !== "string" || raw.length > 32768) return
      var data = JSON.parse(raw || "{}")
      root.isRunning = data.running === true
      root.isPaused = data.paused === true
      root.isStopped = data.stopped === true || !data.running
      root.trackTitle = root.sanitize(data.title || "", 120)
      root.trackArtist = root.sanitize(data.artist || "", 100)
      root.volume = typeof data.volume === "number" ? Math.max(0, Math.min(100, Math.round(data.volume))) : 75
      root.isLiked = data.liked === true
      root.position = Number(data.position || 0)
      root.duration = Number(data.duration || 0)
    } catch (e) {
      // Ignora JSON incompleto durante escritas atômicas
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
    text: root.isStopped ? "🎵" : root.isPaused ? "⏸" : "▶"
    active: root.isRunning && !root.isPaused

    tooltipText: root.isStopped
      ? "YTM-RS (YouTube Music): Inativo\nClique com o botão direito para abrir"
      : (root.isPaused ? "Pausado: " : "Tocando: ")
        + root.sanitize(root.trackArtist ? root.trackArtist + " - " + root.trackTitle : root.trackTitle, 140)
        + (root.isLiked ? " ♥" : "")
        + " · Vol: " + root.volume + "%"

    onPressed: function(mouseButton) {
      if (!root.bar) return

      if (mouseButton === Qt.LeftButton) {
        // Alternar Play / Pause
        root.runCli(["toggle"])
        return
      }

      if (mouseButton === Qt.MiddleButton) {
        // Curtir faixa atual
        root.runCli(["like"])
        return
      }

      if (mouseButton === Qt.RightButton) {
        // Alternar janela TUI
        root.bar.run("hyprctl dispatch togglespecialworkspace ytm || foot --app-id=ytm-rs --title='YTM-RS' -e ytm-rs")
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
}
