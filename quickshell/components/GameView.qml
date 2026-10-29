import QtQuick
import "../state/ThemePalette.js" as ThemePalette

// Stand-in for the game dashboard's content area. The page itself runs in
// its own window (quickshell/game/GameWindow.qml) that the daemon places
// under the header while the live panel shrinks to the header, so this only
// shows in previews or while the panel is full height.
Rectangle {
    id: root

    property var theme: ThemePalette.fallback
    property bool previewMode: false

    color: "transparent"

    Column {
        anchors.centerIn: parent
        spacing: 10

        Text {
            objectName: "gameTitle"
            anchors.horizontalCenter: parent.horizontalCenter
            text: "OH NO! MORE AGENTS"
            textFormat: Text.PlainText
            color: root.theme.cyan
            font {
                family: "monospace"
                pixelSize: 34
                weight: Font.Bold
                letterSpacing: 2
            }
        }

        Text {
            objectName: "gameStatus"
            anchors.horizontalCenter: parent.horizontalCenter
            text: root.previewMode
                ? "THE GAME RUNS IN ITS OWN WINDOW ON THE EDGE"
                : "STARTING THE GAME"
            textFormat: Text.PlainText
            color: ThemePalette.ensureContrast(
                String(root.theme.textMuted),
                String(root.theme.canvas || root.theme.background),
                4.5
            )
            font {
                family: "monospace"
                pixelSize: 15
                weight: Font.DemiBold
                letterSpacing: 1.2
            }
        }
    }
}
