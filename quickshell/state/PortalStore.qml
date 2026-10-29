import QtQml

QtObject {
    id: root

    property int schemaVersion: 1
    property double sequence: -1
    property string daemonEpoch: ""
    property double generatedAtMs: 0
    property bool hasSnapshot: false
    property bool freshSnapshotRequired: true

    property string transportState: "starting"
    property string transportDetail: ""
    property string protocolError: ""

    // The UI consumes a small view model instead of holding the wire envelope.
    // Only public protocol metadata is copied into it.
    property var connection: ({
        "state": "reconnecting",
        "detail": "Waiting for xeneon-agentd"
    })
    property var sessions: []
    property var agents: []
    property var agentOrder: ({
        "available": false,
        "mode": "grouped"
    })
    property var backend: ({
        "mode": "herdr",
        "switchable": false
    })
    // Desktop apps the daemon may launch (desktop.apps, 0-4).
    property var desktopApps: []
    // Enabled dashboards in order; the daemon always lists at least one.
    property var dashboards: ["agents"]
    property var spotify: ({"available": false})
    property var basecamp: ({"available": false})
    property var slack: ({"available": false})
    property var github: ({"available": false})
    property var health: ({})
    property var voice: ({
        "available": false,
        "state": "unavailable",
        "owned": false
    })
    property var usage: ({
        "providers": []
    })
    property var micro: ({
        "connected": false,
        "firmware": "",
        "battery": -1,
        "charging": false,
        "layer": -1,
        "profile": -1,
        "last_updated_ms": 0
    })
    property var lastActionResult: null
    property var lastNotice: null
    property string activitySignature: ""
    property var pendingRequestActions: ({})
    property var pendingRequestOrder: []

    signal snapshotAccepted(var snapshot)
    signal semanticActivityChanged(string signature)
    signal actionResultReceived(var result)
    signal noticeReceived(var notice)
    signal protocolRejected(string reason)

    readonly property var allowedAgentStates: [
        "blocked",
        "done",
        "working",
        "idle",
        "unknown"
    ]

    readonly property var allowedConnectionStates: [
        "connected",
        "degraded",
        "reconnecting",
        "offline"
    ]

    readonly property var allowedSessionStates: [
        "connected",
        "stale",
        "incompatible",
        "offline"
    ]

    readonly property var allowedVoiceStates: [
        "unavailable",
        "idle",
        "recording",
        "processing",
        "error"
    ]

    readonly property var allowedBackends: ["herdr", "t3code"]

    function reset() {
        sequence = -1
        daemonEpoch = ""
        generatedAtMs = 0
        hasSnapshot = false
        freshSnapshotRequired = true
        transportState = "starting"
        transportDetail = ""
        protocolError = ""
        connection = {
            "state": "reconnecting",
            "detail": "Waiting for xeneon-agentd"
        }
        sessions = []
        agents = []
        agentOrder = {"available": false, "mode": "grouped"}
        backend = {"mode": "herdr", "switchable": false}
        desktopApps = []
        dashboards = ["agents"]
        spotify = {"available": false}
        basecamp = {"available": false}
        slack = {"available": false}
        github = {"available": false}
        health = {}
        voice = {
            "available": false,
            "state": "unavailable",
            "owned": false
        }
        usage = {"providers": []}
        micro = {
            "connected": false,
            "firmware": "",
            "battery": -1,
            "charging": false,
            "layer": -1,
            "profile": -1,
            "last_updated_ms": 0
        }
        lastActionResult = null
        lastNotice = null
        activitySignature = ""
        pendingRequestActions = {}
        pendingRequestOrder = []
    }

    function safeString(value, fallback, limit) {
        var normalized = value === null || value === undefined
                ? String(fallback || "")
                : String(value)
        if (limit !== undefined && normalized.length > limit)
            return normalized.slice(0, limit)
        return normalized
    }

    function finiteNumber(value, fallback) {
        var parsed = Number(value)
        return isFinite(parsed) ? parsed : fallback
    }

    function normalizeCapability(value, expectedKind) {
        if (value === null || value === undefined
                || typeof value !== "object")
            return null

        var capabilityId = safeString(value.capability_id, "", 128)
        var kind = safeString(value.kind, "", 24)
        var expiresAtMs = finiteNumber(value.expires_at_ms, -1)
        var expectedRevision = finiteNumber(value.expected_revision, -1)
        if (capabilityId === "" || kind !== expectedKind
                || expiresAtMs < 0 || expectedRevision < 0)
            return null

        return {
            "capability_id": capabilityId,
            "kind": kind,
            "expires_at_ms": expiresAtMs,
            "expected_revision": expectedRevision
        }
    }

    function statePriority(state, reviewReady) {
        switch (state) {
        case "blocked":
            return 0
        case "done":
            return reviewReady === true ? 1 : 3
        case "working":
            return 2
        case "idle":
            return reviewReady === true ? 1 : 3
        default:
            return 4
        }
    }

    function normalizeAgent(value) {
        if (value === null || value === undefined
                || typeof value !== "object")
            return null

        var id = safeString(value.id, "", 128)
        if (id === "")
            return null

        var status = safeString(value.status, "unknown", 24).toLowerCase()
        if (allowedAgentStates.indexOf(status) === -1)
            status = "unknown"

        var rawActions = value.actions !== null
                && value.actions !== undefined
                && typeof value.actions === "object"
                ? value.actions
                : {}

        return {
            "id": id,
            "display_name": safeString(
                value.display_name,
                "Unnamed agent",
                56
            ),
            "agent": safeString(value.agent, "agent", 48),
            "status": status,
            "workspace": safeString(value.workspace, "", 96),
            "repository": safeString(value.repository, "", 96),
            "worktree": safeString(value.worktree, "", 128),
            "branch": safeString(value.branch, "", 64),
            "session": safeString(value.session, "", 64),
            "focused": value.focused === true,
            "launch_pending": value.launch_pending === true,
            "review_ready": value.review_ready === true
                || (value.review_ready === undefined && status === "done"),
            "observed_for_seconds": Math.max(
                0,
                Math.floor(finiteNumber(value.observed_for_seconds, 0))
            ),
            "source_order": Math.max(
                0,
                Math.floor(finiteNumber(value.source_order, 0))
            ),
            "state_change_seq": Math.max(
                0,
                Math.floor(finiteNumber(value.state_change_seq, 0))
            ),
            "actions": {
                "open": rawActions.open === true,
                "zoom": rawActions.zoom === true,
                "approve": normalizeCapability(
                    rawActions.approve,
                    "approve"
                ),
                "interrupt": normalizeCapability(
                    rawActions.interrupt,
                    "interrupt"
                )
            }
        }
    }

    function normalizeAgents(values, orderMode) {
        var normalized = []
        for (var index = 0; index < values.length; index += 1) {
            var agent = normalizeAgent(values[index])
            if (agent !== null)
                normalized.push(agent)
        }

        normalized.sort(function(left, right) {
            if (orderMode === "grouped") {
                if (left.source_order !== right.source_order)
                    return left.source_order - right.source_order
                return left.id.localeCompare(right.id)
            }
            var priorityDifference = statePriority(
                left.status,
                left.review_ready
            ) - statePriority(right.status, right.review_ready)
            if (priorityDifference !== 0)
                return priorityDifference
            if (left.state_change_seq !== right.state_change_seq)
                return right.state_change_seq - left.state_change_seq
            if (left.source_order !== right.source_order)
                return left.source_order - right.source_order
            return left.id.localeCompare(right.id)
        })

        return normalized
    }

    function normalizeAgentOrder(value) {
        var source = value !== null
                && value !== undefined
                && typeof value === "object"
            ? value
            : {}
        var mode = safeString(source.mode, "grouped", 16).toLowerCase()
        if (mode !== "grouped" && mode !== "priority")
            mode = "grouped"
        return {
            "available": source.available === true,
            "mode": mode
        }
    }

    function normalizeBackend(value) {
        var source = value !== null
                && value !== undefined
                && typeof value === "object"
            ? value
            : {}
        var mode = safeString(source.mode, "herdr", 16).toLowerCase()
        if (allowedBackends.indexOf(mode) === -1)
            mode = "herdr"
        return {
            "mode": mode,
            "switchable": source.switchable === true
        }
    }

    function normalizeDashboards(value) {
        var known = ["agents", "spotify", "basecamp", "slack", "github", "game"]
        var result = []
        if (Array.isArray(value)) {
            for (var index = 0; index < value.length; index += 1) {
                var id = safeString(value[index], "", 16)
                if (known.indexOf(id) !== -1 && result.indexOf(id) === -1)
                    result.push(id)
            }
        }
        return result.length > 0 ? result : ["agents"]
    }

    function normalizeSpotify(value) {
        if (value === null || typeof value !== "object" || value.available !== true)
            return {"available": false}
        var status = safeString(value.status, "stopped", 16)
        if (["playing", "paused", "stopped"].indexOf(status) === -1)
            status = "stopped"
        var artUrl = safeString(value.art_url, "", 256)
        var length = Math.max(0, finiteNumber(value.length_ms, 0))
        return {
            "available": true,
            "status": status,
            "title": safeString(value.title, "", 160),
            "artist": safeString(value.artist, "", 160),
            "album": safeString(value.album, "", 160),
            "art_url": artUrl.indexOf("https://i.scdn.co/image/") === 0 ? artUrl : "",
            "length_ms": length,
            "position_ms": Math.min(
                length,
                Math.max(0, finiteNumber(value.position_ms, 0))
            ),
            "can_control": value.can_control === true,
            "can_go_next": value.can_go_next === true,
            "can_go_previous": value.can_go_previous === true
        }
    }

    // Basecamp items carry no URLs; ids are numeric and only ever sent back
    // to the daemon, which owns the links.
    function normalizeBasecamp(value) {
        if (value === null || typeof value !== "object" || value.available !== true)
            return {"available": false}
        var laneRoles = [
            "accent", "red", "orange", "yellow", "green", "blue", "cyan",
            "magenta"
        ]
        var kinds = ["comment", "announcement", "done", "boost", "update"]
        function itemId(id) {
            var text = safeString(id, "", 20)
            return /^[nc][0-9]{1,19}$/.test(text) ? text : ""
        }
        function count(number) {
            return Math.max(0, Math.floor(finiteNumber(number, 0)))
        }
        var notifications = []
        var sourceNotifications = Array.isArray(value.notifications)
            ? value.notifications
            : []
        for (var n = 0; n < sourceNotifications.length && notifications.length < 8; n += 1) {
            var item = sourceNotifications[n] || {}
            var id = itemId(item.id)
            if (id === "")
                continue
            var kind = safeString(item.kind, "update", 16)
            notifications.push({
                "id": id,
                "kind": kinds.indexOf(kind) === -1 ? "update" : kind,
                "title": safeString(item.title, "", 160),
                "project": safeString(item.project, "", 48),
                "creator": safeString(item.creator, "", 48),
                "unread_at": safeString(item.unread_at, "", 40)
            })
        }
        var boards = []
        var sourceBoards = Array.isArray(value.boards) ? value.boards : []
        for (var b = 0; b < sourceBoards.length && b < 6; b += 1) {
            var board = sourceBoards[b] || {}
            var lanes = []
            var sourceLanes = Array.isArray(board.lanes) ? board.lanes : []
            for (var l = 0; l < sourceLanes.length && l < 8; l += 1) {
                var lane = sourceLanes[l] || {}
                var cards = []
                var sourceCards = Array.isArray(lane.cards) ? lane.cards : []
                for (var c = 0; c < sourceCards.length && cards.length < 12; c += 1) {
                    var card = sourceCards[c] || {}
                    var cardId = itemId(card.id)
                    if (cardId === "")
                        continue
                    cards.push({
                        "id": cardId,
                        "title": safeString(card.title, "", 160),
                        "assignees": (Array.isArray(card.assignees) ? card.assignees : [])
                            .slice(0, 4)
                            .map(function(name) { return safeString(name, "", 48) }),
                        "due_on": /^[0-9]{4}-[0-9]{2}-[0-9]{2}$/.test(String(card.due_on || ""))
                            ? String(card.due_on)
                            : "",
                        "comments": count(card.comments)
                    })
                }
                var color = safeString(lane.color, "accent", 16)
                lanes.push({
                    "title": safeString(lane.title, "", 48),
                    "color": laneRoles.indexOf(color) === -1 ? "accent" : color,
                    "total": Math.max(count(lane.total), cards.length),
                    "cards": cards
                })
            }
            boards.push({
                "label": safeString(board.label, "", 24),
                "available": board.available === true,
                "lanes": lanes,
                "not_now": count(board.not_now),
                "done": count(board.done)
            })
        }
        return {
            "available": true,
            "unread_count": count(value.unread_count),
            "unreads_capped": value.unreads_capped === true,
            "notifications": notifications,
            "boards": boards
        }
    }

    // Slack items carry no links or tokens; ids are `<conversation>-<ts>`
    // and only ever sent back to the daemon, which owns the permalinks.
    function normalizeSlack(value) {
        if (value === null || typeof value !== "object" || value.available !== true)
            return {"available": false}
        function itemId(id) {
            var text = safeString(id, "", 48)
            return /^[A-Z0-9]{1,24}-[0-9]{1,12}\.[0-9]{1,8}$/.test(text) ? text : ""
        }
        function count(number) {
            return Math.max(0, Math.floor(finiteNumber(number, 0)))
        }
        var channels = []
        var sourceChannels = Array.isArray(value.channels) ? value.channels : []
        for (var c = 0; c < sourceChannels.length && c < 6; c += 1) {
            var channel = sourceChannels[c] || {}
            var messages = []
            var sourceMessages = Array.isArray(channel.messages) ? channel.messages : []
            for (var m = 0; m < sourceMessages.length && messages.length < 20; m += 1) {
                var message = sourceMessages[m] || {}
                var id = itemId(message.id)
                if (id === "")
                    continue
                var images = []
                var sourceImages = Array.isArray(message.images) ? message.images : []
                for (var i = 0; i < sourceImages.length && images.length < 2; i += 1) {
                    var image = sourceImages[i] || {}
                    var file = safeString(image.file, "", 40)
                    var width = count(image.width)
                    var height = count(image.height)
                    // A bare cache file name only: no paths, no URLs.
                    if (/^F[A-Z0-9]{1,23}\.(png|jpg|jpeg|gif|webp)$/.test(file)
                            && width > 0 && height > 0)
                        images.push({"file": file, "width": width, "height": height})
                }
                var reactions = []
                var sourceReactions = Array.isArray(message.reactions)
                    ? message.reactions
                    : []
                for (var r = 0; r < sourceReactions.length && reactions.length < 8; r += 1) {
                    var reaction = sourceReactions[r] || {}
                    var emoji = safeString(reaction.emoji, "", 34).trim()
                    var times = count(reaction.count)
                    if (emoji !== "" && times > 0)
                        reactions.push({"emoji": emoji, "count": times})
                }
                messages.push({
                    "id": id,
                    "author": safeString(message.author, "", 48),
                    "text": safeString(message.text, "", 500),
                    "images": images,
                    "reactions": reactions,
                    "at": count(message.at),
                    "replies": count(message.replies),
                    "unread": message.unread === true
                })
            }
            channels.push({
                "name": safeString(channel.name, "", 48),
                "available": channel.available === true,
                "unread": count(channel.unread),
                "unread_capped": channel.unread_capped === true,
                "messages": messages
            })
        }
        var notifications = []
        var sourceNotifications = Array.isArray(value.notifications)
            ? value.notifications
            : []
        for (var n = 0; n < sourceNotifications.length && notifications.length < 10; n += 1) {
            var item = sourceNotifications[n] || {}
            var notificationId = itemId(item.id)
            if (notificationId === "")
                continue
            notifications.push({
                "id": notificationId,
                "kind": item.kind === "dm" ? "dm" : "mention",
                "source": safeString(item.source, "", 49),
                "author": safeString(item.author, "", 48),
                "text": safeString(item.text, "", 500),
                "at": count(item.at),
                "unread": count(item.unread)
            })
        }
        return {
            "available": true,
            "channels": channels,
            "notifications": notifications
        }
    }

    // GitHub items carry no URLs; ids are `p<number>` or `n<number>` and only
    // ever sent back to the daemon, which owns the links.
    function normalizeGithub(value) {
        if (value === null || typeof value !== "object" || value.available !== true)
            return {"available": false}
        function itemId(id) {
            var text = safeString(id, "", 41)
            return /^[pn][0-9]{1,20}$/.test(text) ? text : ""
        }
        function count(number) {
            return Math.max(0, Math.floor(finiteNumber(number, 0)))
        }
        var pulls = []
        var sourcePulls = Array.isArray(value.pull_requests) ? value.pull_requests : []
        for (var p = 0; p < sourcePulls.length && pulls.length < 30; p += 1) {
            var pull = sourcePulls[p] || {}
            var pullId = itemId(pull.id)
            if (pullId === "" || pullId[0] !== "p")
                continue
            pulls.push({
                "id": pullId,
                "repo": safeString(pull.repo, "", 64),
                "number": count(pull.number),
                "title": safeString(pull.title, "", 160),
                "author": safeString(pull.author, "", 64),
                "draft": pull.draft === true,
                "comments": count(pull.comments),
                "at": count(pull.at)
            })
        }
        var kinds = ["pr", "issue", "release", "commit", "ci", "discussion", "other"]
        var notifications = []
        var sourceNotifications = Array.isArray(value.notifications)
            ? value.notifications
            : []
        for (var n = 0; n < sourceNotifications.length && notifications.length < 12; n += 1) {
            var item = sourceNotifications[n] || {}
            var notificationId = itemId(item.id)
            if (notificationId === "" || notificationId[0] !== "n")
                continue
            var kind = safeString(item.kind, "other", 16)
            notifications.push({
                "id": notificationId,
                "kind": kinds.indexOf(kind) === -1 ? "other" : kind,
                "title": safeString(item.title, "", 160),
                "repo": safeString(item.repo, "", 128),
                "reason": safeString(item.reason, "", 32),
                "at": count(item.at)
            })
        }
        return {
            "available": true,
            "organization": safeString(value.organization, "", 39),
            "open_pull_requests": count(value.open_pull_requests),
            "pull_requests": pulls,
            "notifications": notifications
        }
    }

    // Only ids, labels, and accent roles the daemon validated are accepted;
    // anything malformed drops that shortcut rather than rendering it.
    function normalizeDesktopApps(value) {
        var allowedAccents = [
            "accent", "green", "blue", "cyan", "magenta", "orange", "yellow"
        ]
        var result = []
        if (!Array.isArray(value))
            return result
        for (var index = 0; index < value.length && result.length < 4;
                index += 1) {
            var app = value[index]
            if (app === null || typeof app !== "object")
                continue
            var id = safeString(app.id, "", 32)
            var label = safeString(app.label, "", 16).trim()
            var accent = safeString(app.accent, "accent", 16)
            if (!/^[a-z0-9-]{1,32}$/.test(id) || label === "")
                continue
            if (result.some(function(existing) { return existing.id === id }))
                continue
            result.push({
                "id": id,
                "label": label,
                "accent": allowedAccents.indexOf(accent) === -1
                    ? "accent"
                    : accent
            })
        }
        return result
    }

    // Human names for the active agent manager. The wire value is the only
    // input; copy never derives from session or agent content.
    function managerName(mode) {
        var normalized = safeString(
            mode === undefined ? backend.mode : mode,
            "herdr",
            16
        ).toLowerCase()
        return normalized === "t3code" ? "T3 Code" : "Herdr"
    }

    function managerLabel(mode) {
        return managerName(mode).toUpperCase()
    }

    function normalizeSessions(values) {
        var normalized = []
        for (var index = 0; index < values.length; index += 1) {
            var session = values[index]
            if (session === null || session === undefined
                    || typeof session !== "object")
                continue

            var name = safeString(session.name, "", 64)
            var state = safeString(session.state, "offline", 24)
                    .toLowerCase()
            if (name === "")
                continue
            if (allowedSessionStates.indexOf(state) === -1)
                state = "offline"

            normalized.push({
                "name": name,
                "state": state,
                "version": safeString(session.version, "", 32),
                "protocol": Math.max(
                    0,
                    Math.floor(finiteNumber(session.protocol, 0))
                ),
                "last_sync_ms": Math.max(
                    0,
                    finiteNumber(session.last_sync_ms, 0)
                ),
                "message": safeString(session.message, "", 160)
            })
        }
        return normalized
    }

    function normalizeConnection(value, sessionViews, backendView) {
        var state = safeString(value, "degraded", 24).toLowerCase()
        if (allowedConnectionStates.indexOf(state) === -1)
            state = "degraded"

        var manager = managerName(
            backendView === undefined ? backend.mode : backendView.mode
        )
        var details = {
            "connected": manager + " state synchronized",
            "degraded": manager + " is reporting partial availability",
            "reconnecting": "Reconnecting to " + manager,
            "offline": manager + " is offline"
        }
        var detail = details[state]
        if (state === "degraded" || state === "reconnecting") {
            for (var index = 0; index < sessionViews.length; index += 1) {
                if (sessionViews[index].state === "incompatible"
                        && sessionViews[index].message !== "") {
                    detail = sessionViews[index].message
                    break
                }
            }
        }
        return {
            "state": state,
            "detail": detail
        }
    }

    function normalizeMetric(value, fallbackUnit) {
        var source = value !== null
                && value !== undefined
                && typeof value === "object"
                ? value
                : {}
        var numericValue = source.value
        var available = source.available === true
                && typeof numericValue === "number"
                && isFinite(numericValue)
        return {
            "available": available,
            "value": available ? numericValue : null,
            "unit": safeString(source.unit, fallbackUnit, 16)
        }
    }

    function normalizeHealth(value) {
        var source = value !== null
                && value !== undefined
                && typeof value === "object"
                ? value
                : {}
        var normalized = {
            "cpu": normalizeMetric(source.cpu, "%"),
            "cpu_temperature": normalizeMetric(
                source.cpu_temperature,
                "°C"
            ),
            "gpu": normalizeMetric(source.gpu, "%"),
            "gpu_temperature": normalizeMetric(
                source.gpu_temperature,
                "°C"
            ),
            "memory": normalizeMetric(source.memory, "%"),
            "network_down": normalizeMetric(source.network_down, "B/s"),
            "network_up": normalizeMetric(source.network_up, "B/s"),
            "battery": normalizeMetric(source.battery, "%")
        }

        var required = [
            "cpu",
            "cpu_temperature",
            "gpu",
            "gpu_temperature",
            "memory",
            "network_down",
            "network_up",
            "battery"
        ]
        normalized.status = "healthy"
        for (var index = 0; index < required.length; index += 1) {
            if (!normalized[required[index]].available) {
                normalized.status = "partial"
                break
            }
        }
        return normalized
    }

    function normalizeVoice(value) {
        var source = value !== null
                && value !== undefined
                && typeof value === "object"
            ? value
            : {}
        var state = safeString(source.state, "unavailable", 24)
                .toLowerCase()
        if (allowedVoiceStates.indexOf(state) === -1)
            state = "error"
        var available = state !== "unavailable"
            && source.available !== false
        if (!available)
            state = "unavailable"
        return {
            "available": available,
            "state": state,
            "owned": available && source.owned === true
        }
    }

    function normalizeUsageWindow(value) {
        if (value === null || value === undefined
                || typeof value !== "object")
            return null
        var utilization = finiteNumber(value.utilization, -1)
        if (utilization < 0)
            return null
        return {
            "label": safeString(value.label, "Usage", 24),
            "utilization": Math.max(0, Math.min(1, utilization)),
            "reset_at_ms": Math.max(
                0,
                finiteNumber(value.reset_at_ms, 0)
            )
        }
    }

    function normalizeAggregateTokens(value) {
        if (typeof value !== "number" || !isFinite(value)
                || value < 0 || value > 1000000000000000)
            return null
        return Math.floor(value)
    }

    function normalizeUsage(value) {
        var source = value !== null
                && value !== undefined
                && typeof value === "object"
            ? value
            : {}
        var providers = Array.isArray(source.providers)
            ? source.providers
            : []
        var normalized = []
        var allowedIds = ["claude", "codex", "opencode"]
        for (var index = 0; index < providers.length; index += 1) {
            var provider = providers[index]
            if (provider === null || provider === undefined
                    || typeof provider !== "object")
                continue
            var id = safeString(provider.id, "", 24).toLowerCase()
            if (allowedIds.indexOf(id) === -1)
                continue
            normalized.push({
                "id": id,
                "label": safeString(provider.label, id, 24),
                "kind": provider.kind === "local_budget"
                    ? "local_budget"
                    : "quota",
                "available": provider.available === true,
                "stale": provider.stale === true,
                "source": safeString(provider.source, "unknown", 24),
                "status": safeString(provider.status, "unknown", 24),
                "plan": safeString(provider.plan, "", 32),
                "model": safeString(provider.model, "", 80),
                "primary": normalizeUsageWindow(provider.primary),
                "secondary": normalizeUsageWindow(provider.secondary),
                "today_tokens": normalizeAggregateTokens(
                    provider.today_tokens
                ),
                "tokens_per_hour": normalizeAggregateTokens(
                    provider.tokens_per_hour
                ),
                "last_updated_ms": Math.max(
                    0,
                    finiteNumber(provider.last_updated_ms, 0)
                )
            })
        }
        normalized.sort(function(left, right) {
            return allowedIds.indexOf(left.id) - allowedIds.indexOf(right.id)
        })
        return {"providers": normalized}
    }

    function normalizeMicro(value) {
        var source = value !== null
                && value !== undefined
                && typeof value === "object"
            ? value
            : {}
        var battery = Math.floor(finiteNumber(source.battery, -1))
        return {
            "connected": source.connected === true,
            "firmware": safeString(source.firmware, "", 32),
            "battery": battery >= 0 && battery <= 100 ? battery : -1,
            "charging": source.charging === true,
            "layer": Math.max(
                -1,
                Math.floor(finiteNumber(source.layer, -1))
            ),
            "profile": Math.max(
                -1,
                Math.floor(finiteNumber(source.profile, -1))
            ),
            "last_updated_ms": Math.max(
                0,
                finiteNumber(source.last_updated_ms, 0)
            )
        }
    }

    function semanticSignature(connectionView, agentViews, voiceView) {
        var fields = [
            backend.mode,
            connectionView.state,
            voiceView.state,
            voiceView.owned === true
        ]
        for (var index = 0; index < agentViews.length; index += 1) {
            fields.push(agentViews[index].id)
            fields.push(agentViews[index].status)
            fields.push(agentViews[index].review_ready === true)
        }
        return JSON.stringify(fields)
    }

    function reject(reason) {
        protocolError = reason
        protocolRejected(reason)
        return false
    }

    function awaitFreshSnapshot(state, detail) {
        transportState = safeString(state, "starting", 24)
        transportDetail = safeString(detail, "", 120)
        freshSnapshotRequired = true
        protocolError = ""
    }

    function trackCommand(command) {
        if (command === null || command === undefined
                || typeof command !== "object")
            return false

        var requestId = safeString(command.request_id, "", 128)
        var action = safeString(command.action, "", 32)
        if (requestId === "" || action === "")
            return false

        var actions = Object.assign({}, pendingRequestActions)
        var order = pendingRequestOrder.slice()
        if (actions[requestId] === undefined)
            order.push(requestId)
        actions[requestId] = action

        while (order.length > 128) {
            var expiredId = order.shift()
            delete actions[expiredId]
        }

        pendingRequestActions = actions
        pendingRequestOrder = order
        return true
    }

    function takeTrackedAction(requestId) {
        var normalizedId = safeString(requestId, "", 128)
        if (normalizedId === ""
                || pendingRequestActions[normalizedId] === undefined)
            return ""

        var actions = Object.assign({}, pendingRequestActions)
        var order = pendingRequestOrder.slice()
        var action = safeString(actions[normalizedId], "", 32)
        delete actions[normalizedId]

        var index = order.indexOf(normalizedId)
        if (index !== -1)
            order.splice(index, 1)
        pendingRequestActions = actions
        pendingRequestOrder = order
        return action
    }

    function ingestEnvelope(envelope) {
        if (envelope === null || envelope === undefined
                || typeof envelope !== "object")
            return reject("Envelope is not an object")

        if (Number(envelope.schema_version) !== schemaVersion)
            return reject("Unsupported schema version")

        if (envelope.type === "snapshot")
            return ingestSnapshot(envelope)
        if (envelope.type === "action_result")
            return ingestActionResult(envelope)
        if (envelope.type === "notice")
            return ingestNotice(envelope)

        return reject("Unsupported envelope type")
    }

    function ingestSnapshot(snapshot) {
        if (!Array.isArray(snapshot.agents)
                || !Array.isArray(snapshot.sessions))
            return reject("Snapshot is missing arrays")

        var nextEpoch = safeString(snapshot.daemon_epoch, "", 128)
        var nextSequence = finiteNumber(snapshot.sequence, -1)
        var nextGeneratedAtMs = finiteNumber(
            snapshot.generated_at_ms !== undefined
                ? snapshot.generated_at_ms
                : snapshot.captured_at_ms,
            0
        )
        if (nextEpoch === "" || nextSequence < 0)
            return reject("Snapshot is missing ordering metadata")

        if (hasSnapshot && nextEpoch === daemonEpoch) {
            if (nextSequence < sequence)
                return false
            if (nextSequence === sequence) {
                if (nextGeneratedAtMs < generatedAtMs)
                    return false
                if (nextGeneratedAtMs > generatedAtMs) {
                    var previousSignature = activitySignature
                    var nextVoice = normalizeVoice(snapshot.voice)
                    var nextUsage = normalizeUsage(snapshot.usage)
                    var nextMicro = normalizeMicro(snapshot.micro)
                    generatedAtMs = nextGeneratedAtMs
                    health = normalizeHealth(snapshot.health)
                    voice = nextVoice
                    usage = nextUsage
                    micro = nextMicro
                    activitySignature = semanticSignature(
                        connection,
                        agents,
                        nextVoice
                    )
                    if (activitySignature !== previousSignature)
                        semanticActivityChanged(activitySignature)
                } else if (!freshSnapshotRequired) {
                    return false
                }
                freshSnapshotRequired = false
                protocolError = ""
                snapshotAccepted(snapshot)
                return true
            }
        }

        var hadSnapshot = hasSnapshot
        var previousSignature = activitySignature
        var nextBackend = normalizeBackend(snapshot.backend)
        var nextSessions = normalizeSessions(snapshot.sessions)
        var nextConnection = normalizeConnection(
            snapshot.connection,
            nextSessions,
            nextBackend
        )
        var nextAgentOrder = normalizeAgentOrder(snapshot.agent_order)
        var nextAgents = normalizeAgents(
            snapshot.agents,
            nextAgentOrder.available ? nextAgentOrder.mode : "priority"
        )
        var nextVoice = normalizeVoice(snapshot.voice)
        var nextUsage = normalizeUsage(snapshot.usage)
        var nextMicro = normalizeMicro(snapshot.micro)
        backend = nextBackend
        desktopApps = normalizeDesktopApps(snapshot.desktop_apps)
        dashboards = normalizeDashboards(snapshot.dashboards)
        spotify = normalizeSpotify(snapshot.spotify)
        basecamp = normalizeBasecamp(snapshot.basecamp)
        slack = normalizeSlack(snapshot.slack)
        github = normalizeGithub(snapshot.github)
        var nextSignature = semanticSignature(
            nextConnection,
            nextAgents,
            nextVoice
        )
        daemonEpoch = nextEpoch
        sequence = nextSequence
        generatedAtMs = nextGeneratedAtMs
        connection = nextConnection
        sessions = nextSessions
        agents = nextAgents
        agentOrder = nextAgentOrder
        health = normalizeHealth(snapshot.health)
        voice = nextVoice
        usage = nextUsage
        micro = nextMicro
        activitySignature = nextSignature
        hasSnapshot = true
        freshSnapshotRequired = false
        protocolError = ""

        snapshotAccepted(snapshot)
        if (hadSnapshot && nextSignature !== previousSignature)
            semanticActivityChanged(nextSignature)
        return true
    }

    function ingestActionResult(envelope) {
        var requestId = safeString(envelope.request_id, "", 128)
        var code = safeString(envelope.code, "", 64)
        if (requestId === "" || code === ""
                || typeof envelope.ok !== "boolean")
            return reject("Action result is incomplete")

        var result = {
            "request_id": requestId,
            "action": takeTrackedAction(requestId),
            "ok": envelope.ok,
            "code": code,
            "message": safeString(envelope.message, "", 120)
        }
        lastActionResult = result
        actionResultReceived(result)
        return true
    }

    function ingestNotice(envelope) {
        var code = safeString(envelope.code, "", 64)
        var level = safeString(envelope.level, "", 24)
        if (code === "" || level === "")
            return reject("Notice is incomplete")

        var notice = {
            "level": level,
            "code": code,
            "message": safeString(envelope.message, "", 120)
        }
        lastNotice = notice
        noticeReceived(notice)
        return true
    }

    function setTransportState(state, detail) {
        transportState = safeString(state, "disconnected", 24)
        transportDetail = safeString(detail, "", 120)
    }

    function surfaceState() {
        if (transportState === "disconnected")
            return "disconnected"
        if (freshSnapshotRequired || !hasSnapshot)
            return "loading"
        if (connection.state === "offline")
            return "disconnected"
        if (connection.state === "degraded"
                || connection.state === "reconnecting"
                || transportState === "degraded"
                || protocolError !== "")
            return "degraded"
        if (agents.length === 0)
            return "empty"
        return "ready"
    }
}
