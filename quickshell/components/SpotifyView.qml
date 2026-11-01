import QtQuick
import QtQuick.Effects
import "../state/ThemePalette.js" as ThemePalette

// Now-playing dashboard. It renders the daemon's Spotify snapshot and
// lyrics and asks for one of three typed controls; it never talks to Spotify
// or the lyrics service itself.
Item {
    id: root

    property var spotify: ({"available": false})
    property var lyrics: ({"status": "off", "title": "", "artist": "", "lines": []})
    property var theme: ThemePalette.fallback
    property bool reducedMotion: false
    property bool canOpenSpotify: false

    signal playPauseRequested()
    signal nextRequested()
    signal previousRequested()
    signal openRequested()
    signal interacted()

    readonly property bool available: spotify !== null
        && spotify !== undefined && spotify.available === true
    readonly property bool playing: available && spotify.status === "playing"
    readonly property color accent: theme.green
    readonly property color readableAccent: ThemePalette.ensureContrast(
        String(accent),
        String(theme.surface),
        4.5
    )
    readonly property color readablePrimary: ThemePalette.ensureContrast(
        String(theme.textPrimary),
        String(theme.surface),
        7
    )
    readonly property color readableMuted: ThemePalette.ensureContrast(
        String(theme.textMuted),
        String(theme.surface),
        4.5
    )

    // The pane shows while lyrics are on; until the daemon has caught up
    // with a track change the lyrics it holds are for the previous track.
    readonly property bool lyricsShown: available && lyrics !== null
        && lyrics !== undefined && lyrics.status !== "off"
    readonly property string lyricsState: !lyricsShown
        ? "off"
        : lyrics.title === spotify.title && lyrics.artist === spotify.artist
            ? lyrics.status
            : "loading"
    // Decorative motion runs only while the dashboard is on screen.
    readonly property bool animated: visible && !reducedMotion
    readonly property bool lyricsReady: (lyricsState === "synced"
        || lyricsState === "plain") && lyricLines.length > 0

    // Every snapshot delivers a new lyrics object; the list is replaced only
    // when the lyrics differ, or it would jump back to the top each second.
    property var lyricLines: []
    property string lyricsKey: ""

    function syncLyrics() {
        // Values set at creation arrive as a list, not a JS array.
        var lines = lyrics && lyrics.lines && lyrics.lines.length !== undefined
            ? Array.prototype.slice.call(lyrics.lines)
            : []
        var key = lyrics
            ? [lyrics.status, lyrics.title, lyrics.artist,
               JSON.stringify(lines)].join("\u0000")
            : ""
        if (key === lyricsKey)
            return
        lyricsKey = key
        lyricLines = lines
        if (lines.length > 0 && animated)
            lyricsIntro.restart()
    }

    onLyricsChanged: syncLyrics()
    // Change handlers do not run for the values a view is created with.
    Component.onCompleted: {
        syncSample()
        syncLyrics()
    }

    // The synced line being sung: the last one that has started, looking a
    // little ahead so the highlight lands with the voice rather than after.
    readonly property int activeLine: {
        if (lyricsState !== "synced")
            return -1
        var lines = lyricLines
        var at = positionMs + 200
        var low = 0
        var high = lines.length - 1
        var found = -1
        while (low <= high) {
            var middle = (low + high) >> 1
            if (Number(lines[middle].at_ms) <= at) {
                found = middle
                low = middle + 1
            } else {
                high = middle - 1
            }
        }
        return found
    }

    // The daemon publishes position only on track, state, or seek changes;
    // between snapshots the bar advances from the last sampled position.
    property double sampledPositionMs: 0
    property double sampledAtMs: Date.now()
    property double nowMs: Date.now()
    readonly property double lengthMs: available ? Number(spotify.length_ms) : 0
    readonly property double positionMs: !available
        ? 0
        : playing
            ? Math.min(lengthMs, sampledPositionMs + nowMs - sampledAtMs)
            : sampledPositionMs
    readonly property real progress: lengthMs > 0 ? positionMs / lengthMs : 0

    // Every daemon snapshot (health refreshes once a second) delivers a new
    // spotify object; only a different sample may re-anchor the bar, or it
    // would snap back to the last sampled position each time.
    property string sampleKey: ""

    onSpotifyChanged: syncSample()

    function syncSample() {
        // Read the new value directly: `available` may not have re-evaluated
        // yet when this handler runs.
        var live = spotify !== null && spotify !== undefined
            && spotify.available === true
        var key = live
            ? [spotify.status, spotify.title, spotify.artist,
               spotify.position_ms, spotify.length_ms].join("\u0000")
            : ""
        if (key === sampleKey)
            return
        sampleKey = key
        sampledPositionMs = live ? Number(spotify.position_ms) : 0
        sampledAtMs = Date.now()
        nowMs = sampledAtMs
    }

    function clock(milliseconds) {
        var totalSeconds = Math.max(0, Math.floor(milliseconds / 1000))
        var minutes = Math.floor(totalSeconds / 60)
        var seconds = totalSeconds % 60
        return minutes + ":" + (seconds < 10 ? "0" : "") + seconds
    }

    Timer {
        interval: root.reducedMotion ? 1000 : 250
        repeat: true
        running: root.playing && root.visible
        onTriggered: root.nowMs = Date.now()
    }

    Rectangle {
        id: card

        objectName: "spotifyCard"
        anchors.fill: parent
        radius: 18
        color: root.theme.surface
        border.width: 1
        border.color: Qt.alpha(root.accent, root.available ? 0.58 : 0.3)
    }

    Rectangle {
        anchors {
            fill: parent
            margins: 4
        }
        radius: 15
        color: "transparent"
        border.width: 1
        border.color: Qt.alpha(root.accent, 0.12)
    }

    Item {
        id: art

        objectName: "spotifyArt"
        anchors {
            left: parent.left
            top: parent.top
            bottom: parent.bottom
            margins: 28
        }
        width: height
        visible: root.available

        Rectangle {
            anchors.fill: parent
            radius: 14
            color: root.theme.surfaceRaised
            border.width: 1
            border.color: Qt.alpha(root.accent, 0.3)
        }

        Image {
            id: artImage

            anchors.fill: parent
            source: root.available ? String(root.spotify.art_url || "") : ""
            fillMode: Image.PreserveAspectCrop
            asynchronous: true
            cache: true
            visible: false
        }

        Rectangle {
            id: artMask

            anchors.fill: parent
            radius: 14
            visible: false
            layer.enabled: true
        }

        MultiEffect {
            anchors.fill: parent
            source: artImage
            maskEnabled: true
            maskSource: artMask
            visible: artImage.status === Image.Ready
        }
    }

    Item {
        id: details

        anchors {
            left: art.right
            right: root.lyricsShown ? lyricsPane.left : parent.right
            top: parent.top
            bottom: parent.bottom
            leftMargin: 44
            rightMargin: 44
            topMargin: 34
            bottomMargin: 30
        }
        visible: root.available

        Row {
            id: statusRow

            spacing: 14

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "NOW PLAYING  //  SPOTIFY"
                textFormat: Text.PlainText
                color: root.readableMuted
                font {
                    family: "monospace"
                    pixelSize: 13
                    weight: Font.DemiBold
                    letterSpacing: 0.8
                }
            }

            Rectangle {
                width: statusText.implicitWidth + 24
                height: 28
                radius: 14
                anchors.verticalCenter: parent.verticalCenter
                color: ThemePalette.mixColors(
                    String(root.theme.surface),
                    String(root.accent),
                    0.14
                )
                border.width: 1
                border.color: Qt.alpha(root.accent, 0.62)

                Text {
                    id: statusText

                    objectName: "spotifyStatus"
                    anchors.centerIn: parent
                    text: root.playing
                        ? "PLAYING"
                        : root.available && root.spotify.status === "paused"
                            ? "PAUSED"
                            : "STOPPED"
                    textFormat: Text.PlainText
                    color: root.readableAccent
                    font {
                        family: "monospace"
                        pixelSize: 12
                        weight: Font.DemiBold
                        letterSpacing: 0.8
                    }
                }
            }

            // A small equalizer that bounces while music plays.
            Row {
                objectName: "spotifyEqualizer"
                anchors.verticalCenter: parent.verticalCenter
                height: 20
                spacing: 3
                visible: root.playing

                Repeater {
                    model: 4

                    delegate: Item {
                        id: bar

                        required property int index
                        property real level: 0.3

                        width: 4
                        height: 20

                        Rectangle {
                            anchors.bottom: parent.bottom
                            width: parent.width
                            height: 4 + 16 * bar.level
                            radius: 2
                            color: root.accent
                        }

                        SequentialAnimation on level {
                            running: root.playing && root.animated
                            loops: Animation.Infinite
                            onRunningChanged: if (!running) bar.level = 0.3

                            NumberAnimation {
                                to: [0.9, 0.6, 1, 0.75][bar.index]
                                duration: [310, 240, 380, 270][bar.index]
                                easing.type: Easing.InOutSine
                            }
                            NumberAnimation {
                                to: [0.15, 0.35, 0.2, 0.1][bar.index]
                                duration: [290, 360, 250, 330][bar.index]
                                easing.type: Easing.InOutSine
                            }
                        }
                    }
                }
            }
        }

        Text {
            id: titleText

            objectName: "spotifyTitle"
            anchors {
                left: parent.left
                right: parent.right
                top: statusRow.bottom
                topMargin: 26
            }
            text: root.available ? String(root.spotify.title || "") : ""
            textFormat: Text.PlainText
            color: root.readablePrimary
            wrapMode: Text.WordWrap
            maximumLineCount: 2
            elide: Text.ElideRight
            font {
                family: "monospace"
                pixelSize: 46
                weight: Font.Bold
            }
        }

        Text {
            id: artistText

            objectName: "spotifyArtist"
            anchors {
                left: parent.left
                right: parent.right
                top: titleText.bottom
                topMargin: 12
            }
            text: root.available ? String(root.spotify.artist || "") : ""
            textFormat: Text.PlainText
            color: root.readableAccent
            elide: Text.ElideRight
            font {
                family: "monospace"
                pixelSize: 24
                weight: Font.DemiBold
                letterSpacing: 0.5
            }
        }

        Text {
            anchors {
                left: parent.left
                right: parent.right
                top: artistText.bottom
                topMargin: 8
            }
            text: root.available ? String(root.spotify.album || "") : ""
            textFormat: Text.PlainText
            color: root.readableMuted
            elide: Text.ElideRight
            font {
                family: "monospace"
                pixelSize: 16
                letterSpacing: 0.3
            }
        }

        Item {
            id: progressBlock

            anchors {
                left: parent.left
                right: parent.right
                bottom: controls.top
                bottomMargin: 30
            }
            height: 32

            Rectangle {
                id: track

                anchors {
                    left: parent.left
                    right: parent.right
                    top: parent.top
                }
                height: 6
                radius: 3
                color: Qt.alpha(root.accent, 0.16)

                Rectangle {
                    objectName: "spotifyProgressFill"
                    width: track.width * root.progress
                    height: parent.height
                    radius: 3
                    color: root.accent
                }
            }

            Text {
                objectName: "spotifyElapsed"
                anchors {
                    left: parent.left
                    bottom: parent.bottom
                }
                text: root.clock(root.positionMs)
                textFormat: Text.PlainText
                color: root.readableMuted
                font {
                    family: "monospace"
                    pixelSize: 14
                    letterSpacing: 0.5
                }
            }

            Text {
                anchors {
                    right: parent.right
                    bottom: parent.bottom
                }
                text: root.clock(root.lengthMs)
                textFormat: Text.PlainText
                color: root.readableMuted
                font {
                    family: "monospace"
                    pixelSize: 14
                    letterSpacing: 0.5
                }
            }
        }

        Row {
            id: controls

            anchors {
                left: parent.left
                bottom: parent.bottom
            }
            spacing: 14

            DesktopAppButton {
                objectName: "spotifyPrevious"
                width: 180
                height: 64
                label: "PREVIOUS"
                detail: "SPOTIFY"
                accent: root.accent
                theme: root.theme
                enabled: root.available && root.spotify.can_control
                    && root.spotify.can_go_previous
                onInteracted: root.interacted()
                onTriggered: root.previousRequested()
            }

            DesktopAppButton {
                objectName: "spotifyPlayPause"
                width: 240
                height: 64
                label: root.playing ? "PAUSE" : "PLAY"
                detail: "SPOTIFY"
                accent: root.accent
                theme: root.theme
                enabled: root.available && root.spotify.can_control
                onInteracted: root.interacted()
                onTriggered: root.playPauseRequested()
            }

            DesktopAppButton {
                objectName: "spotifyNext"
                width: 180
                height: 64
                label: "NEXT"
                detail: "SPOTIFY"
                accent: root.accent
                theme: root.theme
                enabled: root.available && root.spotify.can_control
                    && root.spotify.can_go_next
                onInteracted: root.interacted()
                onTriggered: root.nextRequested()
            }
        }
    }

    Item {
        id: lyricsPane

        objectName: "spotifyLyricsPane"
        anchors {
            right: parent.right
            top: parent.top
            bottom: parent.bottom
            rightMargin: 36
            topMargin: 34
            bottomMargin: 30
        }
        width: Math.round(root.width * 0.42)
        visible: root.lyricsShown

        Rectangle {
            anchors {
                left: parent.left
                top: parent.top
                bottom: parent.bottom
                leftMargin: -22
            }
            width: 1
            color: Qt.alpha(root.accent, 0.2)
        }

        Text {
            id: lyricsLabel

            height: 28
            verticalAlignment: Text.AlignVCenter
            text: root.lyricsState === "plain"
                ? "LYRICS  //  NO TIMING FOR THIS SONG"
                : "LYRICS"
            textFormat: Text.PlainText
            color: root.readableMuted
            font {
                family: "monospace"
                pixelSize: 13
                weight: Font.DemiBold
                letterSpacing: 0.8
            }
        }

        ListView {
            id: lyricList

            objectName: "spotifyLyrics"
            anchors {
                left: parent.left
                right: parent.right
                top: lyricsLabel.bottom
                bottom: parent.bottom
                topMargin: 18
            }
            clip: true
            visible: root.lyricsReady
            model: root.lyricsReady ? root.lyricLines : []
            spacing: 14
            boundsBehavior: Flickable.StopAtBounds
            // Synced lyrics follow the song; plain ones scroll by touch.
            interactive: root.lyricsState === "plain"
            currentIndex: root.lyricsState === "synced"
                ? Math.max(0, root.activeLine)
                : -1
            highlightRangeMode: root.lyricsState === "synced"
                ? ListView.StrictlyEnforceRange
                : ListView.NoHighlightRange
            preferredHighlightBegin: Math.round(height * 0.32)
            preferredHighlightEnd: preferredHighlightBegin
                + (currentItem ? currentItem.height : 40)
            highlightFollowsCurrentItem: true
            highlightMoveDuration: root.reducedMotion ? 0 : 520
            onMovementStarted: {
                touchedAtMs = Date.now()
                root.interacted()
            }
            transform: Translate { id: lyricShift }

            // Untimed lyrics scroll with the song's progress, so they roughly
            // keep up with it, except for a while after a touch.
            property double touchedAtMs: 0

            function followSong() {
                if (moving || Date.now() - touchedAtMs < 8000)
                    return
                var range = contentHeight - height
                if (range <= 0)
                    return
                var target = originY + range * root.progress
                if (!root.animated) {
                    contentY = target
                    return
                }
                plainScroll.to = target
                plainScroll.restart()
            }

            Timer {
                interval: 1000
                repeat: true
                triggeredOnStart: true
                running: root.lyricsState === "plain" && root.lyricsReady
                    && root.visible
                onTriggered: lyricList.followSong()
            }

            NumberAnimation {
                id: plainScroll

                target: lyricList
                property: "contentY"
                duration: 950
                easing.type: Easing.InOutSine
            }

            // New lyrics fade and slide up into place.
            ParallelAnimation {
                id: lyricsIntro

                NumberAnimation {
                    target: lyricList
                    property: "opacity"
                    from: 0
                    to: 1
                    duration: 520
                    easing.type: Easing.OutCubic
                }
                NumberAnimation {
                    target: lyricShift
                    property: "y"
                    from: 36
                    to: 0
                    duration: 620
                    easing.type: Easing.OutCubic
                }
            }

            delegate: Item {
                id: lyricRow

                required property var modelData
                required property int index

                readonly property bool synced: root.lyricsState === "synced"
                readonly property bool current: synced && index === root.activeLine
                readonly property bool gap: String(modelData.text || "") === ""
                readonly property int distance: Math.abs(
                    index - Math.max(0, root.activeLine)
                )

                width: ListView.view.width
                height: Math.ceil(lineBox.height * lineBox.scale)
                opacity: !synced ? 1 : current ? 1 : Math.max(0.3, 0.9 - distance * 0.16)

                Behavior on opacity {
                    enabled: !root.reducedMotion
                    NumberAnimation { duration: 320 }
                }

                // Every line is laid out at the sung size and scaled down
                // when it is not the one being sung, so growing into place
                // is a smooth scale rather than a jump in font size.
                Item {
                    id: lineBox

                    width: lyricRow.width / scale
                    height: label.implicitHeight
                    transformOrigin: Item.TopLeft
                    scale: !lyricRow.synced || lyricRow.current ? 1 : 0.8

                    Behavior on scale {
                        enabled: !root.reducedMotion
                        NumberAnimation {
                            duration: 380
                            easing.type: Easing.OutCubic
                        }
                    }

                    Text {
                        id: label

                        width: parent.width
                        text: !lyricRow.gap
                            ? String(lyricRow.modelData.text)
                            : lyricRow.synced ? "\u25cf  \u25cf  \u25cf" : " "
                        textFormat: Text.PlainText
                        wrapMode: Text.WordWrap
                        color: lyricRow.current ? root.readableAccent : root.readableMuted
                        font {
                            family: "monospace"
                            pixelSize: lyricRow.synced ? 30 : 24
                            weight: Font.DemiBold
                        }

                        Behavior on color {
                            enabled: !root.reducedMotion
                            ColorAnimation { duration: 320 }
                        }
                    }

                    // An instrumental break breathes while it lasts.
                    SequentialAnimation {
                        running: lyricRow.current && lyricRow.gap && root.playing
                            && root.animated
                        loops: Animation.Infinite
                        onRunningChanged: if (!running) lineBox.opacity = 1

                        NumberAnimation {
                            target: lineBox
                            property: "opacity"
                            to: 0.55
                            duration: 700
                            easing.type: Easing.InOutSine
                        }
                        NumberAnimation {
                            target: lineBox
                            property: "opacity"
                            to: 1
                            duration: 700
                            easing.type: Easing.InOutSine
                        }
                    }
                }
            }
        }

        Text {
            id: lyricsStatus

            objectName: "spotifyLyricsStatus"
            anchors {
                left: parent.left
                right: parent.right
                verticalCenter: lyricList.verticalCenter
            }
            visible: root.lyricsShown && !root.lyricsReady
            horizontalAlignment: Text.AlignHCenter

            // Breathes while a lookup is under way.
            SequentialAnimation on opacity {
                running: root.animated && (root.lyricsState === "loading"
                    || root.lyricsState === "unavailable")
                loops: Animation.Infinite
                onRunningChanged: if (!running) lyricsStatus.opacity = 1

                NumberAnimation { to: 0.4; duration: 900; easing.type: Easing.InOutSine }
                NumberAnimation { to: 1; duration: 900; easing.type: Easing.InOutSine }
            }
            text: root.lyricsState === "instrumental"
                ? "INSTRUMENTAL"
                : root.lyricsState === "missing"
                    ? "NO LYRICS FOR THIS TRACK"
                    : root.lyricsState === "unavailable"
                        ? "LYRICS UNAVAILABLE  //  RETRYING"
                        : "LOOKING FOR LYRICS"
            textFormat: Text.PlainText
            color: root.readableMuted
            font {
                family: "monospace"
                pixelSize: 18
                weight: Font.DemiBold
                letterSpacing: 1
            }
        }
    }

    Column {
        objectName: "spotifyOffline"
        anchors.centerIn: parent
        spacing: 14
        visible: !root.available

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: "SPOTIFY  //  OFFLINE"
            textFormat: Text.PlainText
            color: root.readablePrimary
            font {
                family: "monospace"
                pixelSize: 30
                weight: Font.Bold
                letterSpacing: 1.2
            }
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: "Start Spotify on this computer to see what is playing."
            textFormat: Text.PlainText
            color: root.readableMuted
            font {
                family: "monospace"
                pixelSize: 15
                letterSpacing: 0.4
            }
        }

        DesktopAppButton {
            anchors.horizontalCenter: parent.horizontalCenter
            width: 220
            height: 56
            visible: root.canOpenSpotify
            label: "OPEN SPOTIFY"
            accent: root.accent
            theme: root.theme
            onInteracted: root.interacted()
            onTriggered: root.openRequested()
        }
    }
}
