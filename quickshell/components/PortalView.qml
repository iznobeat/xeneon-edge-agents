import QtQuick
import "../state/ThemePalette.js" as ThemePalette

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

    readonly property int pageSize: 14
    // Dashboards the daemon enabled, in order. The selection is a display
    // preference and falls back to the first enabled dashboard.
    readonly property var dashboards: store.dashboards !== undefined
        && store.dashboards.length > 0 ? store.dashboards : ["agents"]
    readonly property string currentDashboard:
        dashboards.indexOf(String(preferences.dashboard || "")) !== -1
            ? String(preferences.dashboard)
            : dashboards[0]
    readonly property bool agentsDashboard: currentDashboard === "agents"
    // The game page runs in its own window under the header; the live panel
    // shrinks to the header while it is shown (see PortalPanel).
    readonly property bool gameShown: currentDashboard === "game"
    readonly property bool headerOnly: gameShown && !paletteSettingsOpen
    readonly property real headerBottom: header.y + header.height
    // Fits "DESKTOP // AGENT COMMAND" (388 px); longer names elide.
    readonly property real identityTextWidth: 400
    // Grouped ordering renders one lane per workspace. Lanes keep the grid's
    // two-row card size and pack whole lanes onto pages of the same width.
    readonly property bool laneLayout:
        agentOrderAvailable && agentOrderMode === "grouped"
    readonly property int laneRows: 2
    readonly property int lanePageColumns: pageSize / laneRows
    readonly property var lanePages: buildLanePages(store.agents)
    readonly property int pageCount: laneLayout
        ? Math.max(1, lanePages.length)
        : Math.max(1, Math.ceil(store.agents.length / pageSize))
    readonly property string surfaceState: store.surfaceState()
    readonly property color degradedColor: theme.needsHelp
    readonly property color readableDegradedColor:
        ThemePalette.ensureContrast(
            String(degradedColor),
            String(theme.canvas),
            4.5
        )
    readonly property color toastColor: toastSuccess
        ? theme.success
        : theme.error
    readonly property color surfaceMessageColor:
        surfaceState === "disconnected"
            ? theme.error
            : theme.accent
    readonly property color readableSurfaceMessageColor:
        ThemePalette.ensureContrast(
            String(surfaceMessageColor),
            String(theme.surface),
            4.5
        )
    readonly property bool userMotionReduced:
        preferences.reduceMotion === true
    readonly property bool displayDimmed:
        preferences.dimmed === true
    readonly property bool effectiveReducedMotion:
        reducedMotion || userMotionReduced
    readonly property bool controlCenterInteractive:
        !activity.ambientMode
        && ambientView.controlCenterProgress >= 0.999
        && !ambientView.exitShield
        && !paletteSettingsOpen
    property int currentPage: 0
    property bool userPaging: false
    property string toastTitle: ""
    property string toastDetail: ""
    property bool toastSuccess: true
    property bool toastVisible: false
    property var pendingDesktopActions: ({})
    property string pendingAgentOrderRequest: ""
    property bool paletteSettingsOpen: false
    property string pendingGameRequest: ""
    readonly property bool agentOrderAvailable:
        store.agentOrder !== undefined
        && store.agentOrder.available === true
    readonly property string agentOrderMode: agentOrderAvailable
        ? String(store.agentOrder.mode)
        : "grouped"
    // The active agent manager is daemon state. QML only renders its name and
    // requests one of two typed switches.
    readonly property string managerMode:
        store.backend !== undefined && store.backend !== null
            && String(store.backend.mode) === "t3code"
            ? "t3code"
            : "herdr"
    readonly property string managerName:
        managerMode === "t3code" ? "T3 Code" : "Herdr"
    readonly property string managerLabel: managerName.toUpperCase()

    clip: true

    function agentAt(pageIndex, slotIndex) {
        var agentIndex = pageIndex * pageSize + slotIndex
        return agentIndex >= 0 && agentIndex < store.agents.length
            ? store.agents[agentIndex]
            : null
    }

    function pageAgentCount(pageIndex) {
        return Math.max(
            0,
            Math.min(pageSize, store.agents.length - pageIndex * pageSize)
        )
    }

    function pageColumns(pageIndex) {
        var count = pageAgentCount(pageIndex)
        if (count <= 2)
            return Math.max(1, count)
        return Math.ceil(count / 2)
    }

    function pageRows(pageIndex) {
        return Math.max(
            1,
            Math.ceil(pageAgentCount(pageIndex) / pageColumns(pageIndex))
        )
    }

    // Agents arrive in the manager's workspace order, so consecutive agents
    // sharing a session and workspace form one lane. A lane too large for a
    // page continues as another lane with the same label.
    function buildLanePages(agents) {
        var laneCapacity = lanePageColumns * laneRows
        var lanes = []
        for (var index = 0; index < agents.length; index += 1) {
            var agent = agents[index]
            var workspace = String(agent.workspace || "").trim()
            var key = String(agent.session || "") + "\u0000" + workspace
            var lane = lanes.length > 0 ? lanes[lanes.length - 1] : null
            if (lane === null || lane.key !== key
                    || lane.agents.length >= laneCapacity) {
                lane = {
                    "key": key,
                    "label": workspace !== "" ? workspace : "Workspace",
                    "agents": []
                }
                lanes.push(lane)
            }
            lane.agents.push(agent)
        }
        var pages = []
        var page = null
        for (var laneIndex = 0; laneIndex < lanes.length; laneIndex += 1) {
            var packed = lanes[laneIndex]
            packed.columns = Math.ceil(packed.agents.length / laneRows)
            if (page === null
                    || page.columns + packed.columns > lanePageColumns) {
                page = { "columns": 0, "rows": 1, "lanes": [] }
                pages.push(page)
            }
            page.lanes.push(packed)
            page.columns += packed.columns
            page.rows = Math.max(
                page.rows,
                Math.min(packed.agents.length, laneRows)
            )
        }
        return pages
    }

    function lanePage(pageIndex) {
        return pageIndex >= 0 && pageIndex < lanePages.length
            ? lanePages[pageIndex]
            : { "columns": 1, "rows": 1, "lanes": [] }
    }

    function dashboardTitle(dashboard) {
        if (dashboard === "spotify")
            return "NOW PLAYING"
        if (dashboard === "basecamp")
            return "BASECAMP"
        if (dashboard === "slack")
            return "SLACK"
        if (dashboard === "github")
            return "GITHUB"
        if (dashboard === "game")
            return "GAME"
        return "AGENT COMMAND"
    }

    function selectDashboard(dashboard) {
        if (dashboards.indexOf(dashboard) === -1)
            return false
        // The AGENTS tab doubles as the order switch: arriving shows
        // Priority, and each further tap flips between Grouped and Priority.
        if (dashboard === "agents") {
            if (agentOrderAvailable && pendingAgentOrderRequest === "") {
                if (agentsDashboard)
                    requestAgentOrder(agentOrderMode === "priority" ? "grouped" : "priority")
                else if (agentOrderMode !== "priority")
                    requestAgentOrder("priority")
            } else if (agentsDashboard) {
                return false
            }
        }
        preferences.dashboard = dashboard
        if (typeof preferences.sync === "function")
            preferences.sync()
        notePassiveInteraction()
        // Opening the Spotify dashboard starts Spotify when it is not running.
        if (dashboard === "spotify" && store.spotify.available !== true
                && desktopAppConfigured("spotify"))
            requestDesktop("spotify")
        return true
    }

    // Starts the game window when needed and places it under the header.
    // Repeated while the game is shown so a closed or crashed window returns.
    function requestGame() {
        if (!gameShown || previewMode || !bridge.ready || pendingGameRequest !== "")
            return false
        var requestId = bridge.showGame()
        if (requestId === "")
            return false
        pendingGameRequest = requestId
        return true
    }

    function dashboardDetail(dashboard) {
        if (dashboard === "spotify") {
            if (store.spotify.available !== true)
                return "OFFLINE"
            return String(store.spotify.status || "stopped").toUpperCase()
        }
        if (dashboard === "basecamp") {
            if (store.basecamp.available !== true)
                return "OFFLINE"
            return store.basecamp.unread_count
                + (store.basecamp.unreads_capped ? "+" : "") + " UNREAD"
        }
        if (dashboard === "slack") {
            if (store.slack.available !== true)
                return "OFFLINE"
            return store.slack.notifications.length + " FOR YOU"
        }
        if (dashboard === "github") {
            if (store.github.available !== true)
                return "OFFLINE"
            return store.github.notifications.length + " UNREAD"
        }
        if (dashboard === "game")
            return "TOUCH TO PLAY"
        var order = pendingAgentOrderRequest !== ""
            ? "SYNCING · "
            : agentOrderAvailable ? agentOrderMode.toUpperCase() + " · " : ""
        var working = 0
        var waiting = 0
        for (var index = 0; index < store.agents.length; index += 1) {
            if (store.agents[index].status === "working")
                working += 1
            else if (store.agents[index].status === "blocked")
                waiting += 1
        }
        return order + (waiting > 0 ? waiting + " WAITING" : working + " WORKING")
    }

    function dashboardAccent(dashboard) {
        if (dashboard === "spotify")
            return theme.green
        if (dashboard === "basecamp")
            return theme.yellow
        if (dashboard === "slack")
            return theme.magenta
        if (dashboard === "github")
            return theme.orange
        if (dashboard === "game")
            return theme.cyan
        return theme.accent
    }

    function desktopAppConfigured(appId) {
        return store.desktopApps.some(function(app) { return app.id === appId })
    }

    function attentionSummary() {
        var working = 0
        var review = 0
        var waiting = 0
        for (var index = 0; index < store.agents.length; index += 1) {
            var agent = store.agents[index]
            if (agent.review_ready === true)
                review += 1
            else if (agent.status === "blocked")
                waiting += 1
            else if (agent.status === "working")
                working += 1
        }
        return working + " WORKING  ·  " + review + " REVIEW  ·  "
            + waiting + " WAITING"
    }

    function notePassiveInteraction() {
        activity.noteUserActivity()
        if (bridge.ready)
            bridge.restoreFocus()
    }

    function selectPage(pageIndex) {
        var nextPage = Math.max(0, Math.min(pageCount - 1, pageIndex))
        currentPage = nextPage
        pages.positionViewAtIndex(nextPage, ListView.SnapPosition)
        notePassiveInteraction()
    }

    function showToast(title, detail, success) {
        toastTitle = String(title || "")
        toastDetail = String(detail || "")
        toastSuccess = success
        toastVisible = true
        toastTimer.restart()
    }

    function desktopActionPending(appId) {
        var requestIds = Object.keys(pendingDesktopActions)
        for (var index = 0; index < requestIds.length; index += 1) {
            if (pendingDesktopActions[requestIds[index]] === appId)
                return true
        }
        return false
    }

    function requestDesktop(appId) {
        if (desktopActionPending(appId))
            return false
        var requestId = bridge.openDesktopApp(appId)
        if (requestId === "")
            return false
        var pending = Object.assign({}, pendingDesktopActions)
        pending[requestId] = appId
        pendingDesktopActions = pending
        return true
    }

    function requestAgentOrder(mode) {
        if (!agentOrderAvailable || pendingAgentOrderRequest !== "")
            return false
        var requestId = bridge.setAgentOrder(mode)
        if (requestId === "")
            return false
        pendingAgentOrderRequest = requestId
        return true
    }

    function toggleDisplayDim() {
        preferences.dimmed = !displayDimmed
        if (typeof preferences.sync === "function")
            preferences.sync()
        return true
    }

    function togglePaletteSettings() {
        activity.noteUserActivity()
        paletteSettingsOpen = !paletteSettingsOpen
        return true
    }

    onPageCountChanged: {
        currentPage = Math.min(currentPage, pageCount - 1)
        pages.positionViewAtIndex(currentPage, ListView.SnapPosition)
    }

    Connections {
        target: root.activity

        function onAmbientModeChanged() {
            if (root.activity.ambientMode)
                root.paletteSettingsOpen = false
        }
    }

    PortalBackground {
        anchors.fill: parent
        theme: root.theme
        reducedMotion: root.effectiveReducedMotion
        ambientMode: root.activity.ambientMode
    }

    Rectangle {
        id: displayDimmer

        objectName: "displayDimmer"
        anchors.fill: parent
        color: "#000000"
        opacity: root.displayDimmed ? 0.88 : 0
        enabled: false
        z: 70

        Behavior on opacity {
            NumberAnimation {
                duration: root.effectiveReducedMotion ? 0 : 220
                easing.type: Easing.OutCubic
            }
        }
    }

    Rectangle {
        objectName: "paletteModalScrim"
        anchors.fill: parent
        visible: root.paletteSettingsOpen
        color: "#000000"
        opacity: 0.46
        z: 68

        Accessible.ignored: true

        TapHandler {
            gesturePolicy: TapHandler.ReleaseWithinBounds
            onTapped: {
                root.activity.noteUserActivity()
                root.paletteSettingsOpen = false
            }
        }
    }

    DisplaySettingsControls {
        id: displaySettings

        objectName: "globalDisplaySettings"
        anchors {
            right: parent.right
            top: parent.top
            rightMargin: 24
            topMargin: 20
        }
        dimmed: root.displayDimmed
        paletteOpen: root.paletteSettingsOpen
        paletteCustom: paletteSettings.customized
        theme: root.theme
        opacity: root.displayDimmed ? 0.58 : 1
        z: 80
        onDimToggleRequested: root.toggleDisplayDim()
        onPaletteToggleRequested: root.togglePaletteSettings()

        Behavior on opacity {
            NumberAnimation {
                duration: root.effectiveReducedMotion ? 0 : 180
                easing.type: Easing.OutCubic
            }
        }
    }

    PaletteSettingsPane {
        id: paletteSettings

        anchors {
            right: parent.right
            top: parent.top
            rightMargin: 24
            topMargin: 82
        }
        open: root.paletteSettingsOpen
        preferences: root.preferences
        sourceTheme: root.sourceTheme
        theme: root.theme
        z: 69
        onCloseRequested: root.paletteSettingsOpen = false
        onInteractionOccurred: root.activity.noteUserActivity()
    }

    Item {
        id: header

        anchors {
            left: parent.left
            right: parent.right
            top: parent.top
            leftMargin: 34
            rightMargin: 34
        }
        height: 92
        opacity: ambientView.controlCenterProgress

        transform: Translate {
            y: (1 - ambientView.controlCenterProgress) * -18
        }

        Row {
            id: headerIdentity

            anchors {
                left: parent.left
                verticalCenter: parent.verticalCenter
            }
            spacing: 18

            Rectangle {
                width: 8
                height: 52
                radius: 4
                color: root.theme.accent
            }

            Column {
                spacing: 2

                Text {
                    objectName: "dashboardTitle"
                    // Fixed width so the tabs never move with the title.
                    width: root.identityTextWidth
                    elide: Text.ElideRight
                    text: String(root.hostName || "LOCAL").toUpperCase()
                        + " // " + root.dashboardTitle(root.currentDashboard)
                    textFormat: Text.PlainText
                    color: root.theme.textPrimary
                    font {
                        family: "monospace"
                        pixelSize: 25
                        weight: Font.Bold
                        letterSpacing: 1.2
                    }
                }

                Text {
                    width: root.identityTextWidth
                    elide: Text.ElideRight
                    text: root.attentionSummary()
                    textFormat: Text.PlainText
                    color: root.theme.textMuted
                    font {
                        family: "monospace"
                        pixelSize: 13
                        letterSpacing: 0.6
                    }
                }
            }
        }

        Row {
            id: headerActions
            objectName: "headerActions"
            anchors {
                left: headerIdentity.right
                leftMargin: 44
                verticalCenter: parent.verticalCenter
            }
            spacing: 12

            // One tab per enabled dashboard, in configured order.
            Repeater {
                model: root.dashboards

                DashboardTab {
                    required property var modelData

                    objectName: "dashboardTab_" + modelData
                    width: 140
                    height: 48
                    label: String(modelData).toUpperCase()
                    detail: root.dashboardDetail(modelData)
                    hint: modelData === "agents" && root.agentOrderAvailable
                        ? (root.agentsDashboard
                            ? "Tap again for " + (root.agentOrderMode === "priority"
                                ? "grouped" : "priority") + " ordering"
                            : "Opens in priority ordering")
                        : ""
                    selected: root.currentDashboard === modelData
                    accent: root.dashboardAccent(modelData)
                    theme: root.theme
                    enabled: root.controlCenterInteractive
                    onTriggered: root.selectDashboard(modelData)
                }
            }


        }

        Row {
            id: pageDots
            objectName: "pageDots"

            // Centered unless the header shortcuts reach the middle.
            x: Math.max(
                (parent.width - width) / 2,
                headerActions.x + headerActions.width + 24
            )
            anchors.verticalCenter: parent.verticalCenter
            spacing: 12

            // Paging still works by swipe when the header has no room left.
            visible: root.agentsDashboard && root.pageCount > 1
                && x + width + 16 <= headerStatus.x

            Repeater {
                model: root.pageCount

                Rectangle {
                    required property int index

                    width: 58
                    height: 48
                    color: "transparent"
                    enabled: root.controlCenterInteractive

                    Accessible.role: Accessible.Button
                    Accessible.ignored: !root.controlCenterInteractive
                    Accessible.name: "Show agent page " + (index + 1)
                        + " of " + root.pageCount
                    Accessible.description: index === root.currentPage
                        ? "Current page"
                        : "Switch pages"
                    Accessible.onPressAction: {
                        if (root.controlCenterInteractive)
                            root.selectPage(index)
                    }

                    Rectangle {
                        anchors.centerIn: parent
                        width: parent.index === root.currentPage ? 46 : 16
                        height: 8
                        radius: 4
                        color: parent.index === root.currentPage
                            ? root.theme.accent
                            : root.theme.border

                        Behavior on width {
                            NumberAnimation {
                                duration: root.effectiveReducedMotion ? 0 : 180
                                easing.type: Easing.OutCubic
                            }
                        }
                    }

                    TapHandler {
                        gesturePolicy: TapHandler.ReleaseWithinBounds
                        onTapped: root.selectPage(parent.index)
                    }
                }
            }
        }

        Row {
            id: headerStatus
            objectName: "headerStatus"
            anchors {
                right: parent.right
                verticalCenter: parent.verticalCenter
                rightMargin: displaySettings.width + 16
            }
            spacing: 12

            ClockCard {
                objectName: "headerClock"
                width: 340
                height: 58
                anchors.verticalCenter: parent.verticalCenter
                theme: root.theme
                reducedMotion: root.effectiveReducedMotion
            }

        }
    }

    Rectangle {
        objectName: "degradedBanner"
        anchors {
            top: header.bottom
            horizontalCenter: parent.horizontalCenter
        }
        width: 960
        height: root.surfaceState === "degraded" ? 28 : 0
        radius: 14
        color: Qt.alpha(root.degradedColor, 0.16)
        border.width: height > 0 ? 1 : 0
        border.color: root.degradedColor
        clip: true
        visible: height > 0
        z: 12
        opacity: ambientView.controlCenterProgress

        Text {
            objectName: "degradedBannerText"
            anchors.centerIn: parent
            text: "DEGRADED // " + String(
                root.store.connection.detail
                    || root.store.protocolError
                    || "PARTIAL TELEMETRY"
            ).toUpperCase()
            textFormat: Text.PlainText
            color: root.readableDegradedColor
            elide: Text.ElideRight
            width: parent.width - 40
            horizontalAlignment: Text.AlignHCenter
            font {
                family: "monospace"
                pixelSize: 12
                weight: Font.DemiBold
                letterSpacing: 0.7
            }
        }

        Behavior on height {
            NumberAnimation {
                duration: root.effectiveReducedMotion ? 0 : 180
            }
        }
    }

    ListView {
        id: pages
        objectName: "agentPages"

        anchors {
            left: parent.left
            right: parent.right
            top: header.bottom
            bottom: commandDock.top
            topMargin: 7
            bottomMargin: 10
        }
        orientation: ListView.Horizontal
        visible: root.agentsDashboard
        model: root.pageCount
        currentIndex: root.currentPage
        snapMode: ListView.SnapOneItem
        boundsBehavior: Flickable.StopAtBounds
        interactive: root.pageCount > 1 && root.surfaceState !== "disconnected"
            && root.controlCenterInteractive
        highlightMoveDuration: root.effectiveReducedMotion ? 0 : 220
        cacheBuffer: width
        clip: true
        opacity: ambientView.controlCenterProgress
        scale: 0.985 + ambientView.controlCenterProgress * 0.015
        transformOrigin: Item.Center

        onMovementStarted: {
            root.userPaging = true
        }

        onMovementEnded: {
            var nextPage = Math.max(
                0,
                Math.min(
                    root.pageCount - 1,
                    Math.round(contentX / width)
                )
            )
            root.currentPage = nextPage
            positionViewAtIndex(nextPage, ListView.SnapPosition)
            if (root.userPaging)
                root.notePassiveInteraction()
            root.userPaging = false
        }

        delegate: Item {
            id: page

            required property int index
            width: pages.width
            height: pages.height

            Grid {
                id: cardGrid
                objectName: "agentGrid_" + page.index

                anchors {
                    fill: parent
                    leftMargin: 34
                    rightMargin: 34
                    topMargin: 8
                    bottomMargin: 4
                }
                columns: root.pageColumns(page.index)
                columnSpacing: 16
                rowSpacing: 16
                visible: !root.laneLayout

                Repeater {
                    model: root.laneLayout ? 0 : root.pageSize

                    PortalAgentCard {
                        required property int index
                        objectName: "agentCard_" + page.index + "_" + index

                        width: (
                            cardGrid.width
                                - cardGrid.columnSpacing
                                    * (cardGrid.columns - 1)
                        ) / cardGrid.columns
                        height: (
                            cardGrid.height
                                - cardGrid.rowSpacing
                                    * (root.pageRows(page.index) - 1)
                        ) / root.pageRows(page.index)
                        agent: root.agentAt(page.index, index)
                    }
                }
            }

            Row {
                id: laneRow
                objectName: "agentLanes_" + page.index

                readonly property var layout: root.lanePage(page.index)
                // One card column; a lane spans its own number of columns.
                readonly property real unitWidth: (
                    width - spacing * (layout.columns - 1)
                ) / Math.max(1, layout.columns)

                anchors {
                    fill: parent
                    leftMargin: 34
                    rightMargin: 34
                    topMargin: 8
                    bottomMargin: 4
                }
                spacing: 16
                visible: root.laneLayout

                Repeater {
                    model: root.laneLayout ? laneRow.layout.lanes.length : 0

                    Column {
                        id: lane

                        required property int index
                        readonly property var laneData:
                            laneRow.layout.lanes[index]

                        objectName: "agentLane_" + page.index + "_" + index
                        width: laneRow.unitWidth * laneData.columns
                            + laneRow.spacing * (laneData.columns - 1)
                        height: laneRow.height
                        spacing: 8

                        Text {
                            id: laneLabel
                            objectName: "agentLaneLabel_" + page.index
                                + "_" + lane.index
                            width: parent.width
                            text: lane.laneData.label.toUpperCase()
                                + "  //  " + lane.laneData.agents.length
                            textFormat: Text.PlainText
                            color: root.theme.textMuted
                            elide: Text.ElideRight
                            font {
                                family: "monospace"
                                pixelSize: 12
                                weight: Font.DemiBold
                                letterSpacing: 0.7
                            }
                        }

                        Grid {
                            id: laneGrid

                            readonly property real cardHeight: (
                                lane.height - laneLabel.height - lane.spacing
                                    - rowSpacing * (laneRow.layout.rows - 1)
                            ) / laneRow.layout.rows

                            columns: lane.laneData.columns
                            columnSpacing: laneRow.spacing
                            rowSpacing: 16

                            Repeater {
                                model: lane.laneData.agents.length

                                PortalAgentCard {
                                    required property int index
                                    objectName: "laneCard_" + page.index
                                        + "_" + lane.index + "_" + index

                                    width: laneRow.unitWidth
                                    height: laneGrid.cardHeight
                                    agent: lane.laneData.agents[index]
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    component PortalAgentCard: AgentCard {
        theme: root.theme
        visible: agent !== null
        enabled: visible && root.controlCenterInteractive
        reducedMotion: root.effectiveReducedMotion
        snapshotSequence: root.store.sequence
        actionsEnabled: !root.store.freshSnapshotRequired
        managerName: root.managerName

        onInteracted: root.activity.noteUserActivity()

        onFocusRequested: function(agentId) {
            // Card activation intentionally focuses the active agent
            // manager's window.
            root.bridge.openAgent(agentId)
        }

        onApproveRequested: function(agentId, capabilityId, sequence) {
            root.bridge.approveAgent(agentId, capabilityId, sequence)
            root.bridge.restoreFocus(sequence)
        }

        onInterruptRequested: function(agentId, capabilityId, sequence) {
            root.bridge.interruptAgent(agentId, capabilityId, sequence)
            root.bridge.restoreFocus(sequence)
        }
    }

    Item {
        anchors {
            left: parent.left
            right: parent.right
            top: pages.top
            bottom: pages.bottom
            margins: 34
        }
        visible: root.agentsDashboard
            && (root.surfaceState === "loading"
                || root.surfaceState === "disconnected"
                || root.surfaceState === "empty")
        z: 20
        opacity: ambientView.controlCenterProgress

        Rectangle {
            anchors.fill: parent
            radius: 22
            color: Qt.alpha(root.theme.surface, 0.92)
            border.width: 1
            border.color: root.surfaceState === "disconnected"
                ? root.theme.error
                : root.theme.border
        }

        Column {
            anchors.centerIn: parent
            spacing: 12

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: root.surfaceState === "loading"
                    ? "SYNCHRONIZING"
                    : root.surfaceState === "disconnected"
                        ? root.managerLabel + " DISCONNECTED"
                        : "NO ACTIVE AGENTS"
                textFormat: Text.PlainText
                color: root.readableSurfaceMessageColor
                font {
                    family: "monospace"
                    pixelSize: 30
                    weight: Font.Light
                    letterSpacing: 4
                }
            }

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: root.surfaceState === "loading"
                    ? "Waiting for the first schema v1 snapshot"
                    : root.surfaceState === "disconnected"
                        ? String(
                            root.store.transportDetail
                                || root.store.connection.detail
                                || "The bridge will reconnect automatically"
                        )
                        : root.managerName
                            + " is connected and has no active agents"
                textFormat: Text.PlainText
                color: root.theme.textMuted
                font {
                    family: "sans-serif"
                    pixelSize: 16
                }
            }
        }
    }

    AiUsageDock {
        id: commandDock

        anchors {
            left: parent.left
            right: parent.right
            bottom: parent.bottom
            leftMargin: 24
            rightMargin: 24
            bottomMargin: 16
        }
        height: 106
        visible: root.agentsDashboard
        usage: root.store.usage
        theme: root.theme
        agents: root.store.agents
        sessions: root.store.sessions
        managerLabel: root.managerLabel
        reducedMotion: root.effectiveReducedMotion
        enabled: root.controlCenterInteractive
        opacity: ambientView.controlCenterProgress
        transform: Translate {
            y: (1 - ambientView.controlCenterProgress) * 14
        }
    }

    Rectangle {
        id: toast

        anchors {
            horizontalCenter: parent.horizontalCenter
            top: parent.top
            topMargin: 82
        }
        width: 780
        height: root.toastVisible ? 74 : 0
        radius: 16
        color: Qt.alpha(root.toastColor, 0.15)
        border.width: root.toastVisible ? 1 : 0
        border.color: root.toastColor
        clip: true
        visible: height > 0
        z: 60
        opacity: ambientView.controlCenterProgress

        Row {
            anchors {
                fill: parent
                margins: 16
            }
            spacing: 16

            Rectangle {
                width: 8
                height: parent.height
                radius: 4
                color: root.toastColor
            }

            Column {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width - 30
                spacing: 3

                Text {
                    width: parent.width
                    text: root.toastTitle
                    textFormat: Text.PlainText
                    color: root.theme.textPrimary
                    elide: Text.ElideRight
                    font {
                        family: "monospace"
                        pixelSize: 15
                        weight: Font.Bold
                        letterSpacing: 0.8
                    }
                }

                Text {
                    width: parent.width
                    text: root.toastDetail
                    textFormat: Text.PlainText
                    color: root.theme.textSecondary
                    elide: Text.ElideRight
                    font {
                        family: "sans-serif"
                        pixelSize: 14
                    }
                }
            }
        }

        Behavior on height {
            NumberAnimation {
                duration: root.effectiveReducedMotion ? 0 : 170
                easing.type: Easing.OutCubic
            }
        }
    }

    Timer {
        id: toastTimer

        interval: 3200
        repeat: false
        onTriggered: root.toastVisible = false
    }

    Connections {
        target: root.store

        function onActionResultReceived(result) {
            if (result.action === "order_grouped"
                    || result.action === "order_priority") {
                root.pendingAgentOrderRequest = ""
                root.showToast(
                    result.ok
                        ? "ORDER // SYNCED"
                        : "ORDER // " + String(result.code).toUpperCase(),
                    result.message || result.request_id,
                    result.ok
                )
                return
            }
            if (result.action === "restore_focus")
                return
            // The game window has no toast: in the live panel only the
            // header is on screen while it is shown.
            if (result.action === "game_show") {
                if (result.request_id === root.pendingGameRequest)
                    root.pendingGameRequest = ""
                if (result.ok && result.message !== "game_shown")
                    gameRetry.restart()
                return
            }
            if (String(result.action).indexOf("spotify_") === 0) {
                if (!result.ok)
                    root.showToast(
                        "SPOTIFY // " + String(result.code).toUpperCase(),
                        result.message || result.request_id,
                        false
                    )
                return
            }
            if (result.action === "desktop_app") {
                var desktopPending = Object.assign(
                    {},
                    root.pendingDesktopActions
                )
                delete desktopPending[result.request_id]
                root.pendingDesktopActions = desktopPending
                root.showToast(
                    result.ok
                        ? "DESKTOP // READY"
                        : "DESKTOP // " + String(result.code).toUpperCase(),
                    result.message || result.request_id,
                    result.ok
                )
                return
            }
            var voiceAction = String(result.action).indexOf("voice_") === 0
            root.showToast(
                result.ok
                    ? voiceAction
                        ? "VOICE // ACCEPTED"
                        : "ACTION // ACCEPTED"
                    : (voiceAction ? "VOICE // " : "ACTION // ")
                        + String(result.code).toUpperCase(),
                result.message || result.request_id,
                result.ok
            )
        }

        function onNoticeReceived(notice) {
            root.showToast(
                "NOTICE // " + String(notice.code).toUpperCase(),
                notice.message,
                String(notice.level).toLowerCase() !== "error"
            )
        }

        function onFreshSnapshotRequiredChanged() {
            if (root.store.freshSnapshotRequired) {
                root.pendingDesktopActions = {}
                root.pendingAgentOrderRequest = ""
                root.pendingGameRequest = ""
            }
        }
    }

    Connections {
        target: root.bridge

        function onCommandRejected(reason) {
            root.showToast("COMMAND REJECTED", reason, false)
        }

        function onCommandEmitted(command) {
            if (root.previewMode) {
                root.showToast(
                    "PREVIEW // " + String(command.action).toUpperCase(),
                    command.request_id,
                    true
                )
            }
        }
    }

    SpotifyView {
        objectName: "spotifyDashboard"
        anchors {
            left: parent.left
            right: parent.right
            top: header.bottom
            bottom: parent.bottom
            leftMargin: 24
            rightMargin: 24
            topMargin: 8
            bottomMargin: 16
        }
        visible: root.currentDashboard === "spotify"
        spotify: root.store.spotify
        theme: root.theme
        reducedMotion: root.effectiveReducedMotion
        canOpenSpotify: root.desktopAppConfigured("spotify")
        enabled: root.controlCenterInteractive
            && !root.store.freshSnapshotRequired
        onInteracted: root.activity.noteUserActivity()
        onPlayPauseRequested: root.bridge.spotifyPlayPause()
        onNextRequested: root.bridge.spotifyNext()
        onPreviousRequested: root.bridge.spotifyPrevious()
        onOpenRequested: root.requestDesktop("spotify")
    }

    BasecampView {
        objectName: "basecampDashboard"
        anchors {
            left: parent.left
            right: parent.right
            top: header.bottom
            bottom: parent.bottom
            leftMargin: 24
            rightMargin: 24
            topMargin: 8
            bottomMargin: 16
        }
        visible: root.currentDashboard === "basecamp"
        basecamp: root.store.basecamp
        theme: root.theme
        enabled: root.controlCenterInteractive
        onInteracted: root.activity.noteUserActivity()
        onOpenRequested: function(itemId) {
            root.bridge.openBasecampItem(itemId)
        }
    }

    SlackView {
        objectName: "slackDashboard"
        anchors {
            left: parent.left
            right: parent.right
            top: header.bottom
            bottom: parent.bottom
            leftMargin: 24
            rightMargin: 24
            topMargin: 8
            bottomMargin: 16
        }
        visible: root.currentDashboard === "slack"
        slack: root.store.slack
        imageDirectory: root.slackImageDirectory
        theme: root.theme
        enabled: root.controlCenterInteractive
        onInteracted: root.activity.noteUserActivity()
        onOpenRequested: function(itemId) {
            root.bridge.openSlackItem(itemId)
        }
    }

    GithubView {
        objectName: "githubDashboard"
        anchors {
            left: parent.left
            right: parent.right
            top: header.bottom
            bottom: parent.bottom
            leftMargin: 24
            rightMargin: 24
            topMargin: 8
            bottomMargin: 16
        }
        visible: root.currentDashboard === "github"
        github: root.store.github
        theme: root.theme
        enabled: root.controlCenterInteractive
        onInteracted: root.activity.noteUserActivity()
        onOpenRequested: function(itemId) {
            root.bridge.openGithubItem(itemId)
        }
    }

    GameView {
        objectName: "gameDashboard"
        anchors {
            left: parent.left
            right: parent.right
            top: header.bottom
            bottom: parent.bottom
        }
        visible: root.currentDashboard === "game"
        theme: root.theme
        previewMode: root.previewMode
    }

    onGameShownChanged: requestGame()

    // A launch or a header resize takes a moment; check back soon, then
    // keep a slow watch while the game is shown.
    Timer {
        id: gameRetry

        interval: 600
        onTriggered: root.requestGame()
    }

    Timer {
        interval: 5000
        repeat: true
        running: root.gameShown && !root.previewMode
        triggeredOnStart: true
        onTriggered: root.requestGame()
    }

    // Other dashboards stay on screen instead of handing over to ambient.
    Binding {
        target: root.activity
        property: "ambientAllowed"
        value: root.agentsDashboard
    }

    AmbientView {
        id: ambientView

        anchors.fill: parent
        active: root.activity.ambientMode
        reducedMotion: root.effectiveReducedMotion
        agents: root.store.agents
        health: root.store.health
        voice: root.store.voice
        connectionState: root.store.connection.state
        theme: root.theme
        onWakeRequested: root.notePassiveInteraction()
    }
}
