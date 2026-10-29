import QtQuick
import QtQuick.Window
import QtWebEngine

// Standalone window for the game dashboard. Quickshell starts Qt without an
// argument list, which QtWebEngine refuses, so the daemon runs this file in
// Qt's own `qml` runtime and Hyprland tiles it under the portal header:
//   qml GameWindow.qml -- <url>
// The page gets the real size of the area and picks its own scale.
Window {
    id: root

    readonly property string defaultUrl: "https://oh-no-more-agents.com/?full&fit"
    readonly property string startUrl: {
        var args = Qt.application.arguments
        var separator = args.indexOf("--")
        var candidate = separator !== -1 && separator + 1 < args.length
            ? String(args[separator + 1]) : ""
        return /^https?:\/\/[^\s]+$/.test(candidate) ? candidate : defaultUrl
    }
    // Navigation stays on the configured site; anything else is dropped.
    readonly property string startOrigin: {
        var match = /^(https?:\/\/[^\/?#]+)/.exec(startUrl)
        return match ? match[1] : ""
    }

    title: "xeneon-edge-game"
    color: "black"
    visible: true
    width: 1280
    height: 400

    WebEngineProfilePrototype {
        id: gameProfile

        // A disk profile keeps the game's localStorage across restarts.
        storageName: "xeneon-game"
    }

    WebEngineView {
        id: view

        anchors.fill: parent
        profile: gameProfile.instance()
        backgroundColor: "black"
        url: root.startUrl

        onNavigationRequested: function(request) {
            var target = String(request.url)
            if (target !== root.startOrigin && target.indexOf(root.startOrigin + "/") !== 0
                    && target.indexOf(root.startOrigin + "?") !== 0)
                request.reject()
        }
        // Long presses on the touchscreen must not open a menu, a new
        // window, or a permission prompt over the board.
        onContextMenuRequested: function(request) { request.accepted = true }
        onNewWindowRequested: function(request) { }
        onPermissionRequested: function(permission) { permission.deny() }
        onRenderProcessTerminated: function(status, exitCode) {
            reloadTimer.restart()
        }
    }

    Timer {
        id: reloadTimer

        interval: 1000
        onTriggered: view.reload()
    }
}
