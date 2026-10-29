import QtQuick

Item {
    id: root

    required property var store
    required property var bridge
    required property var activity
    required property var preferences
    required property var theme
    property var sourceTheme: theme
    property bool reducedMotion: false
    property bool previewMode: false
    property string hostName: "LOCAL"
    property string slackImageDirectory: ""

    readonly property real designWidth: 2560
    readonly property real designHeight: 720
    // True while only the header belongs on screen (the game dashboard); the
    // panel then shrinks to headerHeight and the design keeps its width scale.
    readonly property bool headerOnly: portalView.headerOnly
    // Header height in logical pixels at a width-fitted scale, rounded up to
    // whole device pixels so the window below it starts on a pixel row.
    readonly property int headerHeight: {
        var logical = portalView.headerBottom * width / designWidth
        var ratio = devicePixelRatio > 0 ? devicePixelRatio : 1
        for (var height = Math.ceil(logical); height < Math.ceil(logical) + 8; height += 1) {
            if (Math.abs(height * ratio - Math.round(height * ratio)) < 0.001)
                return height
        }
        return Math.ceil(logical)
    }
    property real devicePixelRatio: 1
    readonly property real viewportScale: headerOnly
        ? width / designWidth
        : Math.min(width / designWidth, height / designHeight)

    clip: true

    Rectangle {
        anchors.fill: parent
        color: root.theme.canvas
    }

    Item {
        id: designSurface

        width: root.designWidth
        height: root.designHeight
        x: (root.width - width * root.viewportScale) / 2
        y: root.headerOnly ? 0 : (root.height - height * root.viewportScale) / 2
        scale: root.viewportScale
        transformOrigin: Item.TopLeft

        PortalView {
            id: portalView

            anchors.fill: parent
            store: root.store
            bridge: root.bridge
            activity: root.activity
            preferences: root.preferences
            theme: root.theme
            sourceTheme: root.sourceTheme
            reducedMotion: root.reducedMotion
            previewMode: root.previewMode
            hostName: root.hostName
            slackImageDirectory: root.slackImageDirectory
        }
    }
}
