import QtQml

QtObject {
    id: root

    readonly property var allowedActions: [
        "open",
        "zoom",
        "approve",
        "interrupt",
        "restore_focus",
        "desktop_app",
        "spotify_play_pause",
        "spotify_next",
        "spotify_previous",
        "basecamp_open",
        "slack_open",
        "github_open",
        "game_show",
        "voice_start",
        "voice_stop",
        "voice_cancel",
        "order_grouped",
        "order_priority",
        "backend_herdr",
        "backend_t3code"
    ]

    property int requestCounter: 0
    property string lastError: ""

    function reset() {
        requestCounter = 0
        lastError = ""
    }

    function build(action, agentId, capabilityId, pinnedSequence, appId, itemId) {
        lastError = ""

        if (allowedActions.indexOf(action) === -1) {
            lastError = "Action is not allowlisted"
            return null
        }

        var normalizedAgentId = String(agentId || "")
        var normalizedCapabilityId = String(capabilityId || "")
        var normalizedAppId = String(appId || "")
        var normalizedItemId = String(itemId || "")
        var normalizedSequence = Number(pinnedSequence)

        if (!isFinite(normalizedSequence) || normalizedSequence < 0) {
            lastError = "Command requires a valid snapshot sequence"
            return null
        }

        var systemAction = action === "restore_focus"
            || action === "desktop_app"
            || action === "spotify_play_pause"
            || action === "spotify_next"
            || action === "spotify_previous"
            || action === "basecamp_open"
            || action === "slack_open"
            || action === "github_open"
            || action === "game_show"
            || action === "voice_start"
            || action === "voice_stop"
            || action === "voice_cancel"
            || action === "order_grouped"
            || action === "order_priority"
            || action === "backend_herdr"
            || action === "backend_t3code"

        if (systemAction) {
            if (normalizedAgentId !== ""
                    || normalizedCapabilityId !== "") {
                lastError = "System action cannot target an agent or capability"
                return null
            }
        } else if (normalizedAgentId === "") {
            lastError = "Agent action requires an agent_id"
            return null
        }

        if ((action === "approve" || action === "interrupt")
                && normalizedCapabilityId === "") {
            lastError = "Guarded action requires a capability_id"
            return null
        }

        if ((action === "open" || action === "zoom" || systemAction)
                && normalizedCapabilityId !== "") {
            lastError = "Direct action cannot include a capability_id"
            return null
        }

        if (action === "desktop_app"
                && !/^[a-z0-9-]{1,32}$/.test(normalizedAppId)) {
            lastError = "Desktop app action requires a configured app_id"
            return null
        }

        if (action !== "desktop_app" && normalizedAppId !== "") {
            lastError = "Only desktop_app accepts an app_id"
            return null
        }

        if (action === "basecamp_open"
                && !/^[nc][0-9]{1,19}$/.test(normalizedItemId)) {
            lastError = "Basecamp open requires a published item_id"
            return null
        }

        if (action === "slack_open"
                && !/^[A-Z0-9]{1,24}-[0-9]{1,12}\.[0-9]{1,8}$/.test(normalizedItemId)) {
            lastError = "Slack open requires a published item_id"
            return null
        }

        if (action === "github_open"
                && !/^[pn][0-9]{1,20}$/.test(normalizedItemId)) {
            lastError = "GitHub open requires a published item_id"
            return null
        }

        if (action !== "basecamp_open" && action !== "slack_open"
                && action !== "github_open" && normalizedItemId !== "") {
            lastError = "Only basecamp_open, slack_open, and github_open accept an item_id"
            return null
        }

        requestCounter += 1
        var command = {
            "schema_version": 1,
            "type": "command",
            "request_id": "xep-" + Date.now() + "-" + requestCounter,
            "sequence": Math.floor(normalizedSequence),
            "action": action
        }

        if (normalizedAgentId !== "")
            command.agent_id = normalizedAgentId

        if (normalizedCapabilityId !== "")
            command.capability_id = normalizedCapabilityId

        if (normalizedAppId !== "")
            command.app_id = normalizedAppId

        if (normalizedItemId !== "")
            command.item_id = normalizedItemId

        return command
    }
}
