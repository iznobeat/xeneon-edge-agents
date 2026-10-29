import QtQuick
import QtTest
import "../components"
import "../state/ThemePalette.js" as ThemePalette

TestCase {
    id: testCase

    name: "PortalLayout"
    width: 2560
    height: 720

    QtObject {
        id: mockStore

        signal actionResultReceived(var result)
        signal noticeReceived(var notice)

        property var agents: []
        property var agentOrder: ({"available": true, "mode": "grouped"})
        property var backend: ({"mode": "herdr", "switchable": true})
        property var desktopApps: []
        property var dashboards: ["agents"]
        property var spotify: ({"available": false})
        property var basecamp: ({"available": false})
        property var slack: ({"available": false})
        property var github: ({"available": false})
        property var sessions: []
        property var usage: ({"providers": []})
        property var micro: ({"connected": false, "charging": false})
        property var voice: ({"available": false, "state": "unavailable"})
        property var health: ({})
        property var connection: ({"state": "connected", "detail": ""})
        property string transportDetail: ""
        property string protocolError: ""
        property bool freshSnapshotRequired: false
        property double sequence: 1
        property string testSurfaceState: ""

        function surfaceState() {
            return testSurfaceState || (agents.length > 0 ? "ready" : "empty")
        }
    }

    QtObject {
        id: mockBridge

        signal commandRejected(string reason)
        signal commandEmitted(var command)

        property bool ready: true
        property int orderRequests: 0
        property string lastOrder: ""
        property var appRequests: ({})

        function restoreFocus() { return "restore" }
        function openAgent(agentId) { return agentId }
        function approveAgent() { return "approve" }
        function interruptAgent() { return "interrupt" }
        function openDesktopApp(appId) {
            var next = Object.assign({}, appRequests)
            next[appId] = (next[appId] || 0) + 1
            appRequests = next
            return appId + "-" + next[appId]
        }
        property var spotifyCommands: []
        property var basecampOpens: []
        property var slackOpens: []
        property var githubOpens: []
        property int gameRequests: 0
        function showGame() {
            gameRequests += 1
            return "game-" + gameRequests
        }
        function openGithubItem(itemId) {
            githubOpens = githubOpens.concat([itemId])
            return "go"
        }
        function openSlackItem(itemId) {
            slackOpens = slackOpens.concat([itemId])
            return "so"
        }
        function openBasecampItem(itemId) {
            basecampOpens = basecampOpens.concat([itemId])
            return "bo"
        }
        function spotifyPlayPause() { spotifyCommands = spotifyCommands.concat(["play_pause"]); return "sp" }
        function spotifyNext() { spotifyCommands = spotifyCommands.concat(["next"]); return "sn" }
        function spotifyPrevious() { spotifyCommands = spotifyCommands.concat(["previous"]); return "sv" }
        function startVoice() { return "voice-start" }
        function stopVoice() { return "voice-stop" }
        function cancelVoice() { return "voice-cancel" }
        function setAgentOrder(mode) {
            orderRequests += 1
            lastOrder = mode
            return "order-" + orderRequests
        }
    }

    QtObject {
        id: mockActivity

        property bool ambientMode: false
        property bool ambientAllowed: true
        property int noteCount: 0
        function noteUserActivity() {
            noteCount += 1
            ambientMode = false
        }
    }

    QtObject {
        id: mockPreferences

        property bool reduceMotion: false
        property bool dimmed: false
        property string dashboard: "agents"
        property string readyColorRole: "muted"
        property string successColorRole: "green"
        property string workingColorRole: "blue"
        property string needsHelpColorRole: "yellow"
        property string reviewReadyColorRole: "green"
        property string errorColorRole: "red"
        property string unknownColorRole: "magenta"
        property string recordingColorRole: "green"
        property string processingColorRole: "cyan"
        property int syncCount: 0
        function sync() { syncCount += 1 }
    }

    PortalView {
        id: portal

        width: testCase.width
        height: testCase.height
        store: mockStore
        bridge: mockBridge
        activity: mockActivity
        preferences: mockPreferences
        theme: ThemePalette.fallback
        reducedMotion: true
    }

    function agent(index) {
        return {
            "id": "layout-agent-" + index,
            "display_name": "Layout Agent " + index,
            "agent": "fixture",
            "status": index % 3 === 0 ? "working" : "idle",
            "review_ready": false,
            "focused": index === 0,
            "observed_for_seconds": index,
            "workspace": "layout-test",
            "repository": "xeneon-edge-agents",
            "branch": "main",
            "worktree": "",
            "actions": {
                "open": true,
                "zoom": true,
                "approve": null,
                "interrupt": null
            }
        }
    }

    function agents(count) {
        var result = []
        for (var index = 0; index < count; index += 1)
            result.push(agent(index))
        return result
    }

    function init() {
        mockStore.agents = agents(20)
        mockStore.agentOrder = {"available": true, "mode": "grouped"}
        mockStore.backend = {"mode": "herdr", "switchable": true}
        mockStore.connection = {"state": "connected", "detail": ""}
        mockStore.testSurfaceState = ""
        mockBridge.orderRequests = 0
        mockBridge.lastOrder = ""
        mockBridge.appRequests = ({})
        mockBridge.spotifyCommands = []
        mockBridge.basecampOpens = []
        mockStore.basecamp = {"available": false}
        mockStore.slack = {"available": false}
        mockBridge.slackOpens = []
        mockStore.github = {"available": false}
        mockBridge.githubOpens = []
        mockBridge.gameRequests = 0
        portal.pendingGameRequest = ""
        mockStore.dashboards = ["agents"]
        mockStore.spotify = {"available": false}
        mockPreferences.dashboard = "agents"
        mockStore.desktopApps = [
            {"id": "spotify", "label": "Spotify", "accent": "green"},
            {"id": "discord", "label": "Discord", "accent": "blue"}
        ]
        portal.pendingDesktopActions = ({})
        portal.pendingAgentOrderRequest = ""
        portal.paletteSettingsOpen = false
        mockPreferences.readyColorRole = "muted"
        mockPreferences.successColorRole = "green"
        mockPreferences.workingColorRole = "blue"
        mockPreferences.needsHelpColorRole = "yellow"
        mockPreferences.reviewReadyColorRole = "green"
        mockPreferences.errorColorRole = "red"
        mockPreferences.unknownColorRole = "magenta"
        mockPreferences.recordingColorRole = "green"
        mockPreferences.processingColorRole = "cyan"
        mockPreferences.syncCount = 0
        mockActivity.ambientMode = false
        mockActivity.noteCount = 0
        portal.currentPage = 0
        wait(0)
    }

    function cleanup() {
        mockStore.agents = []
        portal.currentPage = 0
        portal.paletteSettingsOpen = false
        wait(0)
    }

    function workspaceAgents(sizes) {
        var result = []
        for (var lane = 0; lane < sizes.length; lane += 1) {
            for (var index = 0; index < sizes[lane]; index += 1) {
                var value = agent(result.length)
                value.workspace = "space-" + lane
                result.push(value)
            }
        }
        return result
    }

    function test_groupedOrderRendersOneLanePerWorkspace() {
        mockStore.agents = workspaceAgents([2, 1, 3])
        wait(0)
        verify(portal.laneLayout)
        compare(portal.pageCount, 1)
        var page = portal.lanePage(0)
        compare(page.lanes.length, 3)
        compare(page.columns, 4)
        compare(page.rows, 2)
        compare(page.lanes[2].columns, 2)

        var grid = findChild(portal, "agentGrid_0")
        var lanes = findChild(portal, "agentLanes_0")
        verify(grid !== null)
        verify(lanes !== null)
        compare(findChild(portal, "agentCard_0_0"), null)
        compare(findChild(portal, "agentLaneLabel_0_0").text, "SPACE-0  //  2")
        compare(findChild(portal, "agentLaneLabel_0_2").text, "SPACE-2  //  3")
        var first = findChild(portal, "agentLane_0_0")
        var last = findChild(portal, "agentLane_0_2")
        verify(last.x + last.width <= lanes.width + 0.5)
        verify(last.width > first.width * 1.9)
        var lastCard = findChild(portal, "laneCard_0_2_2")
        verify(lastCard !== null)
        compare(lastCard.agent.id, "layout-agent-5")
        verify(lastCard.height > 200)
    }

    function test_lanesPackWholeWorkspacesOntoPages() {
        // 6 + 6 + 4 agents need 3 + 3 + 2 columns; only 7 fit on a page.
        mockStore.agents = workspaceAgents([6, 6, 4])
        wait(0)
        compare(portal.pageCount, 2)
        compare(portal.lanePage(0).lanes.length, 2)
        compare(portal.lanePage(0).columns, 6)
        compare(portal.lanePage(1).lanes.length, 1)
        compare(portal.lanePage(1).lanes[0].label, "space-2")
    }

    function test_workingCardsShowABottomTrackThatHoldsStillWhenReduced() {
        mockStore.agentOrder = {"available": true, "mode": "priority"}
        mockStore.agents = agents(2)
        wait(0)
        // agent(0) is working, agent(1) is idle; reducedMotion is on here.
        var workingCard = findChild(portal, "agentCard_0_0")
        var idleCard = findChild(portal, "agentCard_0_1")
        verify(workingCard.working)
        verify(!idleCard.working)
        var track = findChild(workingCard, "cardWorkingTrack")
        var glider = findChild(workingCard, "cardWorkingGlider")
        verify(track !== null && glider !== null)
        verify(track.y + track.height <= workingCard.height)
        compare(glider.x, (track.width - glider.width) / 2)
    }

    function test_dashboardTabsSelectDashboardsAndHideAgentControls() {
        var title = findChild(portal, "dashboardTitle")
        // A lone dashboard still gets its tab: it is the order switch.
        verify(findChild(portal, "dashboardTab_agents") !== null)
        verify(portal.agentsDashboard)

        mockStore.dashboards = ["agents", "spotify"]
        mockStore.spotify = {"available": true, "status": "paused"}
        wait(0)
        var agentsTab = findChild(portal, "dashboardTab_agents")
        var spotifyTab = findChild(portal, "dashboardTab_spotify")
        verify(agentsTab.selected && !spotifyTab.selected)
        compare(spotifyTab.label, "SPOTIFY")
        compare(spotifyTab.detail, "PAUSED")

        verify(spotifyTab.activate())
        compare(portal.currentDashboard, "spotify")
        compare(mockPreferences.dashboard, "spotify")
        verify(spotifyTab.selected && !agentsTab.selected)
        verify(title.text.indexOf("// NOW PLAYING") > 0)
        verify(!mockActivity.ambientAllowed)

        verify(agentsTab.activate())
        compare(portal.currentDashboard, "agents")
        verify(mockActivity.ambientAllowed)

        // A remembered dashboard that is no longer enabled falls back.
        mockPreferences.dashboard = "spotify"
        mockStore.dashboards = ["agents"]
        wait(0)
        compare(portal.currentDashboard, "agents")
    }

    function test_openingSpotifyDashboardStartsSpotifyOnlyWhenOffline() {
        mockStore.dashboards = ["agents", "spotify"]
        wait(0)
        var spotifyTab = findChild(portal, "dashboardTab_spotify")
        compare(spotifyTab.detail, "OFFLINE")
        verify(spotifyTab.activate())
        compare(mockBridge.appRequests.spotify, 1)

        verify(findChild(portal, "dashboardTab_agents").activate())
        mockStore.spotify = {"available": true, "status": "playing"}
        mockStore.actionResultReceived({
            "request_id": "spotify-1", "action": "desktop_app", "ok": true,
            "code": "desktop_app_launched", "message": "desktop_app_launched"
        })
        verify(spotifyTab.activate())
        compare(mockBridge.appRequests.spotify, 1)

        // Without a configured spotify app nothing is launched.
        verify(findChild(portal, "dashboardTab_agents").activate())
        mockStore.spotify = {"available": false}
        mockStore.desktopApps = []
        verify(spotifyTab.activate())
        compare(mockBridge.appRequests.spotify, 1)
    }

    function test_gameDashboardShowsOnlyTheHeaderAndPlacesItsWindow() {
        mockStore.dashboards = ["agents", "game"]
        wait(0)
        var gameTab = findChild(portal, "dashboardTab_game")
        compare(gameTab.label, "GAME")
        compare(gameTab.detail, "TOUCH TO PLAY")
        compare(portal.headerOnly, false)

        verify(gameTab.activate())
        compare(portal.currentDashboard, "game")
        compare(portal.headerOnly, true)
        compare(portal.headerBottom, 92)
        verify(findChild(portal, "gameDashboard") !== null)
        compare(findChild(portal, "gameStatus").text, "STARTING THE GAME")
        compare(mockBridge.gameRequests, 1)

        // One request at a time; the window is checked again after a launch.
        verify(!portal.requestGame())
        compare(mockBridge.gameRequests, 1)
        mockStore.actionResultReceived({
            "request_id": "game-1", "action": "game_show", "ok": true,
            "code": "ok", "message": "game_launching"
        })
        compare(portal.pendingGameRequest, "")
        tryCompare(mockBridge, "gameRequests", 2)

        // The palette needs the full panel even over the game.
        portal.paletteSettingsOpen = true
        compare(portal.headerOnly, false)
        portal.paletteSettingsOpen = false

        verify(findChild(portal, "dashboardTab_agents").activate())
        compare(portal.headerOnly, false)
        compare(portal.currentDashboard, "agents")
    }

    function test_dashboardTabsOrderAndPageDotsFitBesideTheStatusRow() {
        var actions = findChild(portal, "headerActions")
        var dots = findChild(portal, "pageDots")
        var status = findChild(portal, "headerStatus")
        mockStore.dashboards = ["agents", "basecamp", "spotify", "slack"]
        mockStore.agents = agents(20)
        mockStore.agentOrder = {"available": true, "mode": "priority"}
        tryCompare(portal, "pageCount", 2)
        var tab = findChild(portal, "dashboardTab_spotify")
        // Headless windows report the portal invisible, so the Row may not
        // lay out items that appear later; add up the slots instead.
        var tabs = portal.dashboards.length * (tab.width + actions.spacing)
        var headerEnd = actions.x + tabs
        verify(headerEnd + 24 + dots.width < status.x)
    }

    function test_spotifyDashboardShowsTrackAndSendsTypedControls() {
        mockStore.dashboards = ["agents", "spotify"]
        mockPreferences.dashboard = "spotify"
        mockStore.spotify = {
            "available": true,
            "status": "paused",
            "title": "Track",
            "artist": "Artist",
            "album": "Album",
            "art_url": "",
            "length_ms": 200000,
            "position_ms": 65000,
            "can_control": true,
            "can_go_next": true,
            "can_go_previous": false
        }
        wait(0)
        compare(findChild(portal, "spotifyTitle").text, "Track")
        compare(findChild(portal, "spotifyStatus").text, "PAUSED")
        compare(findChild(portal, "spotifyElapsed").text, "1:05")
        var playPause = findChild(portal, "spotifyPlayPause")
        compare(playPause.label, "PLAY")
        verify(playPause.activate())
        verify(findChild(portal, "spotifyNext").activate())
        verify(!findChild(portal, "spotifyPrevious").activate())
        compare(mockBridge.spotifyCommands, ["play_pause", "next"])

        mockStore.spotify = {"available": false}
        wait(0)
        verify(!findChild(portal, "spotifyTitle").parent.visible)
    }

    function test_spotifyProgressKeepsRunningAcrossUnrelatedSnapshots() {
        mockStore.dashboards = ["agents", "spotify"]
        mockPreferences.dashboard = "spotify"
        var playing = {
            "available": true, "status": "playing", "title": "Track",
            "artist": "Artist", "album": "", "art_url": "",
            "length_ms": 200000, "position_ms": 10000,
            "can_control": true, "can_go_next": true, "can_go_previous": true
        }
        mockStore.spotify = Object.assign({}, playing)
        var view = findChild(portal, "spotifyDashboard")
        wait(0)
        // Pretend the last sample was taken 30 s ago.
        view.sampledAtMs = view.sampledAtMs - 30000
        // A health refresh republishes identical Spotify values.
        mockStore.spotify = Object.assign({}, playing)
        wait(0)
        verify(view.positionMs >= 40000)
        // A seek reported by the daemon does re-anchor.
        mockStore.spotify = Object.assign({}, playing, {"position_ms": 5000})
        wait(0)
        verify(view.positionMs < 6000)
    }

    function basecampSnapshot() {
        function lane(title, color, count) {
            var cards = []
            for (var index = 0; index < count; index += 1)
                cards.push({
                    "id": "c" + (100 + index), "title": title + " card " + index,
                    "assignees": ["Kari Nordmann"], "due_on": "", "comments": 1
                })
            return {"title": title, "color": color, "total": count + 3, "cards": cards}
        }
        return {
            "available": true, "unread_count": 30, "unreads_capped": false,
            "notifications": [{
                "id": "n555", "kind": "comment", "title": "Re: Plan",
                "project": "Produkt", "creator": "Ola",
                "unread_at": new Date(Date.now() - 2 * 3600000).toISOString()
            }],
            "boards": [
                {"label": "Sprint", "available": true, "not_now": 11, "done": 106,
                 "lanes": [lane("Triage", "accent", 2), lane("Under arbeid", "yellow", 8)]},
                {"label": "Bugs", "available": true, "not_now": 0, "done": 4,
                 "lanes": [lane("Open", "red", 1)]}
            ]
        }
    }

    function test_basecampDashboardShowsLanesAndOpensItemsById() {
        mockStore.dashboards = ["agents", "basecamp"]
        mockPreferences.dashboard = "basecamp"
        mockStore.basecamp = basecampSnapshot()
        wait(0)
        verify(findChild(portal, "dashboardTitle").text.indexOf("// BASECAMP") > 0)
        compare(findChild(portal, "basecampUnread").text, "HEY!  //  30 UNREAD")
        compare(findChild(portal, "basecampParked").text, "NOT NOW // 11    DONE // 106")
        var lane = findChild(portal, "basecampLane_1")
        verify(lane !== null)
        // Cards that do not fit are counted, never squeezed.
        verify(lane.shown >= 1 && lane.shown <= 8)
        verify(lane.shown * 92 <= lane.height)

        var card = findChild(portal, "basecampCard_0_1")
        verify(card.activate())
        var item = findChild(portal, "basecampNotification_0")
        verify(item.activate())
        compare(mockBridge.basecampOpens, ["c101", "n555"])

        var view = findChild(portal, "basecampDashboard")
        verify(findChild(portal, "basecampBoardTab_1").activate())
        compare(view.boardIndex, 1)
        compare(findChild(portal, "basecampParked").text, "NOT NOW // 0    DONE // 4")

        mockStore.basecamp = {"available": false}
        wait(0)
        verify(view.boards.length === 0)
    }

    function test_slackDashboardShowsChannelFeedAndOpensItemsById() {
        mockStore.dashboards = ["agents", "slack"]
        mockPreferences.dashboard = "slack"
        var now = Math.floor(Date.now() / 1000)
        mockStore.slack = {
            "available": true,
            "channels": [
                {"name": "dev", "available": true, "unread": 1, "unread_capped": false,
                 "messages": [
                     {"id": "CDEV-1700000050.000100", "author": "Jon", "text": "older",
                      "at": now - 600, "replies": 0, "unread": false,
                      "images": [], "reactions": []},
                     {"id": "CDEV-1700000200.000300", "author": "Kari",
                      "text": "hi @Jon see docs", "at": now - 60, "replies": 3, "unread": true,
                      "images": [], "reactions": [{"emoji": "👍", "count": 3}]}
                 ]},
                {"name": "support", "available": false, "unread": 0,
                 "unread_capped": false, "messages": []}
            ],
            "notifications": [
                {"id": "D1-1700000300.000200", "kind": "dm", "source": "Kari",
                 "author": "Kari", "text": "lunch?", "at": now - 120, "unread": 2}
            ]
        }
        wait(0)
        verify(findChild(portal, "dashboardTitle").text.indexOf("// SLACK") > 0)
        compare(findChild(portal, "dashboardTab_slack").detail, "1 FOR YOU")
        compare(findChild(portal, "slackInboxTitle").text, "SLACK  //  1 FOR YOU")
        var feed = findChild(portal, "slackMessages")
        compare(feed.count, 2)
        var newest = findChild(portal, "slackMessage_1")
        verify(newest !== null)
        verify(findChild(newest, "slackReactions_1") !== null)
        compare(newest.modelData.reactions[0].emoji, "👍")
        compare(findChild(portal, "slackMessage_0").modelData.reactions.length, 0)
        verify(newest.activate())
        verify(findChild(portal, "slackNotification_0").activate())
        compare(mockBridge.slackOpens, ["CDEV-1700000200.000300", "D1-1700000300.000200"])

        var view = findChild(portal, "slackDashboard")
        verify(findChild(portal, "slackChannelTab_1").activate())
        compare(view.channelIndex, 1)
        compare(view.messages.length, 0)
    }

    function test_githubDashboardShowsPullRequestsAndNotificationsAndOpensById() {
        mockStore.dashboards = ["agents", "github"]
        mockPreferences.dashboard = "github"
        var now = Math.floor(Date.now() / 1000)
        mockStore.github = {
            "available": true, "organization": "favo",
            "open_pull_requests": 44,
            "pull_requests": [
                {"id": "p9001", "repo": "app", "number": 42, "title": "Fix login",
                 "author": "kari", "draft": false, "comments": 3, "at": now - 300},
                {"id": "p9002", "repo": "web", "number": 7, "title": "WIP header",
                 "author": "ola", "draft": true, "comments": 0, "at": now - 7200}
            ],
            "notifications": [
                {"id": "n123", "kind": "pr", "title": "Fix login", "repo": "favo/app",
                 "reason": "review_requested", "at": now - 600}
            ]
        }
        wait(0)
        verify(findChild(portal, "dashboardTitle").text.indexOf("// GITHUB") > 0)
        compare(findChild(portal, "dashboardTab_github").detail, "1 UNREAD")
        compare(findChild(portal, "githubFeedTitle").text, "FAVO  //  44 OPEN PULL REQUESTS")
        compare(findChild(portal, "githubInboxTitle").text, "GITHUB  //  1 UNREAD")
        compare(findChild(portal, "githubPullRequests").count, 2)
        verify(findChild(portal, "githubPullRequest_0").activate())
        verify(findChild(portal, "githubNotification_0").activate())
        compare(mockBridge.githubOpens, ["p9001", "n123"])
    }

    function test_priorityOrderKeepsTheFlatGrid() {
        mockStore.agents = workspaceAgents([2, 1, 3])
        mockStore.agentOrder = {"available": true, "mode": "priority"}
        wait(0)
        verify(!portal.laneLayout)
        verify(findChild(portal, "agentCard_0_0") !== null)
        compare(findChild(portal, "agentLane_0_0"), null)
    }

    function test_twentyAgentsUseFourteenPlusSixPages() {
        mockStore.agentOrder = {"available": true, "mode": "priority"}
        wait(0)
        compare(portal.pageSize, 14)
        compare(portal.pageCount, 2)
        compare(portal.pageAgentCount(0), 14)
        compare(portal.pageColumns(0), 7)
        compare(portal.pageRows(0), 2)
        compare(portal.pageAgentCount(1), 6)
        compare(portal.pageColumns(1), 3)
        compare(portal.pageRows(1), 2)
        compare(portal.agentAt(0, 13).id, "layout-agent-13")
        compare(portal.agentAt(1, 0).id, "layout-agent-14")
        compare(portal.agentAt(1, 5).id, "layout-agent-19")
        compare(portal.agentAt(1, 6), null)

        var firstGrid = findChild(portal, "agentGrid_0")
        var firstLastCard = findChild(portal, "agentCard_0_13")
        var firstSpaceName = findChild(firstLastCard, "agentSpaceName")
        verify(firstGrid !== null)
        verify(firstLastCard !== null)
        verify(firstSpaceName !== null)
        compare(firstGrid.columns, 7)
        compare(firstSpaceName.text, "XENEON-EDGE-AGENTS  ·  MAIN")
        verify(!firstSpaceName.truncated)
        verify(firstLastCard.width > 330)
        verify(firstLastCard.height > 220)
        verify(firstLastCard.x + firstLastCard.width <= firstGrid.width + 0.5)
        verify(firstLastCard.y + firstLastCard.height <= firstGrid.height + 0.5)

        portal.selectPage(1)
        tryCompare(portal, "currentPage", 1)
        var pages = findChild(portal, "agentPages")
        tryCompare(pages, "currentIndex", 1)
        var secondGrid = findChild(portal, "agentGrid_1")
        var secondLastCard = findChild(portal, "agentCard_1_5")
        verify(secondGrid !== null)
        verify(secondLastCard !== null)
        compare(secondGrid.columns, 3)
        verify(secondLastCard.x + secondLastCard.width <= secondGrid.width + 0.5)
        verify(secondLastCard.y + secondLastCard.height <= secondGrid.height + 0.5)
    }

    function test_agentsTabShowsPriorityFirstThenTogglesOrderOnce() {
        mockStore.dashboards = ["agents", "spotify"]
        mockStore.spotify = {"available": true, "status": "paused"}
        mockPreferences.dashboard = "spotify"
        wait(0)
        var tab = findChild(portal, "dashboardTab_agents")
        compare(tab.detail.indexOf("GROUPED"), 0)

        // Arriving on the agent dashboard asks for Priority first.
        verify(tab.activate())
        compare(portal.currentDashboard, "agents")
        compare(mockBridge.orderRequests, 1)
        compare(mockBridge.lastOrder, "priority")
        compare(tab.detail.indexOf("SYNCING"), 0)
        // A tap while the request is in flight does nothing.
        tab.activate()
        compare(mockBridge.orderRequests, 1)

        mockStore.actionResultReceived({
            "request_id": "order-1",
            "action": "order_priority",
            "ok": true,
            "code": "agent_order_updated",
            "message": "agent_order_updated"
        })
        compare(portal.pendingAgentOrderRequest, "")
        mockStore.agentOrder = {"available": true, "mode": "priority"}
        compare(tab.detail.indexOf("PRIORITY"), 0)

        // Each further tap flips the order.
        verify(tab.activate())
        compare(mockBridge.orderRequests, 2)
        compare(mockBridge.lastOrder, "grouped")
    }

    function test_headerTabsKeepTheirPlaceWhateverTheTitleLength() {
        var actions = findChild(portal, "headerActions")
        var x = actions.x
        portal.hostName = "a-very-long-hostname-that-would-push-things"
        wait(0)
        compare(actions.x, x)
        portal.hostName = "X"
        wait(0)
        compare(actions.x, x)
        verify(findChild(portal, "dashboardTitle").truncated === false)
        // A real host name and the agent title fit without eliding.
        portal.hostName = "desktop"
        wait(0)
        verify(findChild(portal, "dashboardTitle").truncated === false)
        portal.hostName = "LOCAL"
    }

    function test_lowerCountsAndShrinkClampSafely() {
        mockStore.agents = agents(4)
        wait(0)
        compare(portal.pageCount, 1)
        compare(portal.pageColumns(0), 2)
        compare(portal.pageRows(0), 2)

        mockStore.agents = agents(20)
        wait(0)
        portal.selectPage(1)
        tryCompare(portal, "currentPage", 1)
        mockStore.agents = agents(6)
        tryCompare(portal, "pageCount", 1)
        tryCompare(portal, "currentPage", 0)
        compare(portal.pageColumns(0), 3)
        compare(portal.pageRows(0), 2)
    }

    function test_degradedBannerRendersBoundedProtocolReasonAsPlainText() {
        var prefix = "Herdr protocol 19 is incompatible <img src='file:///private'> "
        var reason = prefix + "X".repeat(160 - prefix.length)
        compare(reason.length, 160)
        mockStore.connection = {"state": "degraded", "detail": reason}
        mockStore.testSurfaceState = "degraded"
        tryCompare(portal, "surfaceState", "degraded")

        var banner = findChild(portal, "degradedBanner")
        var label = findChild(portal, "degradedBannerText")
        verify(banner !== null)
        verify(label !== null)
        tryCompare(banner, "height", 28)
        compare(label.text, "DEGRADED // " + reason.toUpperCase())
        compare(label.textFormat, Text.PlainText)
        verify(label.truncated)
    }

    function test_palettePaneMapsAllowlistedThemeRolesAndResets() {
        var controls = findChild(portal, "globalDisplaySettings")
        var toggle = findChild(controls, "paletteSettingButton")
        var pane = findChild(portal, "paletteSettingsPane")
        var scrim = findChild(portal, "paletteModalScrim")
        var dimmer = findChild(portal, "displayDimmer")
        verify(controls !== null)
        verify(toggle !== null)
        verify(pane !== null)
        verify(scrim !== null)
        verify(dimmer !== null)
        compare(toggle.stateLabel, "THEME")
        verify(toggle.activate())
        compare(portal.paletteSettingsOpen, true)
        compare(pane.open, true)
        compare(toggle.stateLabel, "OPEN")
        compare(mockActivity.noteCount, 1)
        verify(pane.x >= 0)
        verify(pane.y >= 0)
        verify(pane.x + pane.width <= portal.width)
        verify(pane.y + pane.height <= portal.height)
        verify(scrim.z < pane.z)
        verify(pane.z < dimmer.z)

        var lastChoice = findChild(
            pane, "paletteRole_processing_magenta"
        )
        verify(lastChoice !== null)
        verify(lastChoice.x + lastChoice.width <= pane.width)
        verify(lastChoice.y + lastChoice.height <= pane.height)

        var redWorking = findChild(pane, "paletteRole_working_red")
        verify(redWorking !== null)
        verify(redWorking.activate())
        compare(mockPreferences.workingColorRole, "red")
        compare(mockPreferences.syncCount, 1)
        compare(mockActivity.noteCount, 2)
        compare(pane.customized, true)

        var close = findChild(pane, "paletteCloseButton")
        verify(close.activate())
        compare(portal.paletteSettingsOpen, false)
        compare(toggle.stateLabel, "CUSTOM")

        portal.paletteSettingsOpen = true
        var reset = findChild(pane, "paletteResetButton")
        verify(reset.enabled)
        verify(reset.activate())
        compare(mockPreferences.workingColorRole, "blue")
        compare(mockPreferences.syncCount, 2)
        compare(pane.customized, false)
        compare(pane.setMapping("working", "not-a-theme-role"), false)
        compare(mockPreferences.workingColorRole, "blue")

        mockPreferences.workingColorRole = "not-a-theme-role"
        compare(pane.customized, true)
        compare(pane.selectionFor("working", "blue"), "blue")
        verify(reset.activate())
        compare(mockPreferences.workingColorRole, "blue")
    }

    function test_palettePaneWakesAmbientAndClosesBeforeReentry() {
        mockActivity.ambientMode = true
        wait(0)
        verify(portal.togglePaletteSettings())
        compare(portal.paletteSettingsOpen, true)
        compare(mockActivity.ambientMode, false)
        compare(mockActivity.noteCount, 1)

        mockActivity.ambientMode = true
        tryCompare(portal, "paletteSettingsOpen", false)
    }
}
