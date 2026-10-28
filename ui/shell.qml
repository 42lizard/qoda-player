//@ pragma AppId qoda-player
import QtQuick
import QtQuick.Window
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import Quickshell.Hyprland

ShellRoot {
    id: root
    property string form: "full"
    property string lastForm: "full"
    UiRequests { id: uiRequests }
    QuitState {
        id: quitState
        connected: backend.connected
        instance: root.instance
        onSendQuit: root.send("quit")
        onExitUi: Qt.quit()
    }
    OmarchyTheme {}
    readonly property var selectedTrack: queueModel.find(row => row.selected)?.track || snapshot.selection || null
    function showSurface(next, interactive) {
        if (next === "compact") {
            const monitor = Hyprland.focusedMonitor ? Hyprland.focusedMonitor.name : "";
            compact.targetScreen = Quickshell.screens.find(screen => screen.name === monitor) || Quickshell.screens[0] || null;
            compact.interactive = !!interactive;
        }
        form = next;
        if (next !== "hidden") lastForm = next;
        compact.visible = next === "compact";
        if (next === "full" && interactive) Qt.callLater(() => {
            if (window.contentItem.Window.window) window.contentItem.Window.window.requestActivate();
        });
    }
    function hideSurface() { showSurface("hidden", false); }
    function toggleSurface() { if (form === "hidden") showSurface(lastForm, true); else hideSurface(); }
    property var snapshot: ({albums: {page: null, selected: null, busy: false, message: I18n.t("Choose a library to browse albums.")}, libraries: [], status: I18n.t("Connecting backend"), intent: "paused", playlists: {list: null, selected: null, busy: false, uncertain: false, message: ""}})
    property var libraryModel: []
    property var trackModel: []
    readonly property bool playbackAvailable: snapshot.playback_available !== undefined ? snapshot.playback_available : !!snapshot.sign_in && snapshot.sign_in.connected
    readonly property bool localSelected: libraryModel.some(library => library.id === libraryId && library.kind === "local")
    property string localRevision: ""
    readonly property bool trackPageReady: (!snapshot.catalog || !snapshot.catalog.cache || snapshot.catalog.cache.origin !== "offline") && backend.connected && !!snapshot.page && !!snapshot.catalog && !snapshot.catalog.busy && snapshot.page.source === libraryId && snapshot.page.query === trackQuery && playbackAvailable
    onTrackPageReadyChanged: if (!trackPageReady) trackSelection.clear()
    TrackSelection {
        id: trackSelection
        tracks: root.trackModel
        revision: (root.snapshot.account_epoch || "0") + ":" + (root.snapshot.catalog ? root.snapshot.catalog.revision : "0")
    }
    function bulkTracks(op) {
        if (!trackPageReady || !trackSelection.count) return;
        editQueue(op, {tracks: trackSelection.references, catalog_revision: snapshot.catalog.revision});
    }

    readonly property bool reordering: queueReorder.active || playlistReorder.active
    ReorderState {
        id: queueReorder
        kind: "queue"; account: root.snapshot.account_epoch || "0"; instance: root.instance
        fence: root.snapshot.queue ? {queue_session: root.snapshot.queue.session, queue_revision: root.snapshot.queue.revision} : null
        rows: root.queueModel
        enabled: backend.connected && root.form === "full" && !settingsSheet.visible && queueTabs.currentIndex === 0 && (queueSlot.visible || queueDrawer.opened)
        onRequested: (op, fields) => root.send(op, fields)
    }
    ReorderState {
        id: playlistReorder
        kind: "playlist"; account: root.snapshot.account_epoch || "0"; instance: root.instance
        fence: root.playlistView ? {reference: root.playlistView.playlist.reference, expected_revision: root.playlistView.revision} : null
        rows: root.playlistEntries
        enabled: root.playlistWritable && root.form === "full" && !settingsSheet.visible && libraryTabs.currentIndex === 1
        onRequested: (op, fields) => root.send(op, fields)
    }
    ReorderScroll { controller: queueReorder; viewport: queueView }
    ReorderScroll { controller: playlistReorder; viewport: playlistTracks }
    property var queueModel: []
    property string queueJson: "[]"
    property string libraryJson: "[]"
    property string trackJson: "[]"
    property var playlistModel: []
    property var playlistEntries: []
    property var playlistSeed: null
    property bool navigatingArtist: false
    property string albumQuery: ""
    property string artistQuery: ""
    property string trackQuery: ""
    property string globalQuery: ""
    property string settingsSection: ""
    readonly property bool audioExpanded: settingsSection === "audio"
    readonly property bool cacheExpanded: settingsSection === "cache"
    property bool cacheBudgetInitialized: false
    property int cacheBudgetDraft: 1024
    property bool metadataBudgetInitialized: false
    property int metadataBudgetDraft: 256
    readonly property bool accountExpanded: settingsSection === "account"
    property bool pendingSearchFocus: false
    function showSettings(section) {
        pendingSearchFocus = false;
        queueRequested = false;
        queueReorder.cancel(); playlistReorder.cancel();
        settingsSection = section;
        showSurface("full", true);
    }
    function closeSettings() { settingsSection = ""; libraryPicker.dismiss(); }
    function openQueue(tab) {
        pendingSearchFocus = false; closeSettings();
        queueRequested = true; queueTabs.currentIndex = tab;
        showSurface("full", true);
    }
    function focusSearch() {
        showSurface("full", true);
        pendingSearchFocus = true;
        closeSettings(); queueRequested = false;
        playerMenu.close();
        libraryTabs.currentIndex = 4;
        Qt.callLater(tryFocusSearch);
    }
    function tryFocusSearch() {
        if (!pendingSearchFocus || form !== "full" || !window.visible || settingsSheet.visible || queueDrawer.visible || playerMenu.visible || libraryPicker.popupVisible) return;
        search.forceActiveFocus(); pendingSearchFocus = false;
    }
    onFormChanged: if (form !== "full") pendingSearchFocus = false
    property bool queueRequested: false
    property string playlistJson: "[]"
    property string playlistEntriesJson: "[]"
    property var playlistView: snapshot.playlists ? snapshot.playlists.selected : null
    readonly property bool playlistWritable: backend.connected && !!snapshot.sign_in && snapshot.sign_in.connected && !!playlistView && playlistView.playlist.can_edit && !playlistView.playlist.smart && !!snapshot.playlists && !snapshot.playlists.busy && !snapshot.playlists.uncertain
    function playlistEdit(edit) {
        if (playlistView) send("playlist_edit", {reference: playlistView.playlist.reference, expected_revision: playlistView.revision, edit: edit});
    }
    function playlistQueue(op, item) {
        if (!playlistView) return;
        let fields = {reference: playlistView.playlist.reference, expected_revision: playlistView.revision};
        if (item) fields.item_id = item;
        editQueue(op, fields);
    }
    function refreshPlaylists() { if (libraryId && !localSelected) send("playlist_refresh", {source: libraryId}); }

    property string libraryId: ""
    property string instance: ""
    function send(op, fields) { backend.send(op, fields); }
    function editQueue(op, fields) { backend.send(op, fields, true); }
    function moveEntry(index, delta) {
        let target = index + delta;
        if (target < 0 || target >= queueModel.length || !queueModel[index].can_move || !queueModel[target].can_move) return;
        editQueue("queue_move", {occurrence: queueModel[index].id, before: delta < 0 ? queueModel[target].id : (target + 1 < queueModel.length ? queueModel[target + 1].id : null)});
    }
    function pageOffset(value, delta) {
        // Decimal strings preserve the socket's u64 contract without JS Number rounding.
        let digits = value.split("");
        let carry = delta;
        for (let i = digits.length - 1; i >= 0; --i) {
            let n = Number(digits[i]) + carry;
            digits[i] = String((n % 10 + 10) % 10);
            carry = Math.floor(n / 10);
        }
        if (carry < 0) return "0";
        while (carry > 0) { digits.unshift(String(carry % 10)); carry = Math.floor(carry / 10); }
        return digits.join("").replace(/^0+(?=.)/, "");
    }
    function browseAlbums(offset) {
        if (!libraryId) return;
        let page = snapshot.albums ? snapshot.albums.page : null;
        if (page && page.source === libraryId && page.artist) send("artist_open", {reference: page.artist.reference, offset: offset || "0", query: albumQuery});
        else send("album_list", {source: libraryId, offset: offset || "0", query: albumQuery});
    }
    function browseArtists(offset) { if (libraryId) send("artist_list", {source: libraryId, offset: offset || "0", query: artistQuery}); }
    function openArtist(reference) {
        searchDelay.stop(); navigatingArtist = true; albumQuery = ""; search.text = ""; libraryTabs.currentIndex = 2;
        send("artist_open", {reference: reference, offset: "0", query: ""}); navigatingArtist = false;
    }
    function allAlbums() { searchDelay.stop(); albumQuery = ""; search.text = ""; if (libraryId) send("album_list", {source: libraryId, offset: "0", query: ""}); }
    function rememberQuery() { if (libraryTabs.currentIndex === 2) albumQuery = search.text; else if (libraryTabs.currentIndex === 3) artistQuery = search.text; else if (libraryTabs.currentIndex === 4) globalQuery = search.text; else trackQuery = search.text; }
    function searchCatalog() { rememberQuery(); if (libraryTabs.currentIndex === 2) browseAlbums("0"); else if (libraryTabs.currentIndex === 3) browseArtists("0"); else if (libraryTabs.currentIndex === 4) send("search_all", {query: globalQuery}); else browse("0"); }
    function browse(offset) {
        if (libraryId) send("browse", {source: libraryId, offset: offset || "0", query: search.text});
    }
    Process { id: browser }
    BackendConnection {
        id: backend
        role: "shell"
        onReset: { uiRequests.reset(); root.instance = ""; }
        onReceived: next => {
            const previousAccount = root.snapshot.account_epoch;
            const previousTrackOffset = root.snapshot.page ? root.snapshot.page.offset : "0";
            if (!root.instance && next.albums && next.albums.page) {
                root.libraryId = next.albums.page.source;
                root.albumQuery = next.albums.page.query;
                search.text = root.albumQuery;
            }
            if (!root.instance && !root.libraryId && next.albums && next.albums.artists) root.libraryId = next.albums.artists.source;
            if (!root.instance && next.page && !root.libraryId) {
                root.libraryId = next.page.source;
                root.trackQuery = next.page.query;
                search.text = libraryTabs.currentIndex === 0 ? root.trackQuery : root.albumQuery;
            }
            if (!next.libraries || next.libraries.length === 0) { root.libraryId = ""; search.text = ""; root.albumQuery = ""; root.artistQuery = ""; root.trackQuery = ""; root.playlistSeed = null; }
            if (root.instance !== next.instance) { root.cacheBudgetInitialized = false; root.metadataBudgetInitialized = false; }
            if (!root.metadataBudgetInitialized && next.metadata_cache && next.metadata_cache.state !== "opening") { root.metadataBudgetDraft = next.metadata_cache.budget_mib; root.metadataBudgetInitialized = true; }
            if (!root.cacheBudgetInitialized && next.artwork_cache) { root.cacheBudgetDraft = next.artwork_cache.budget_mib; root.cacheBudgetInitialized = true; }
            const raised = uiRequests.consume(next.instance, next.ui ? next.ui.raise_token : "");
            root.instance = next.instance;
            root.snapshot = next;
            if (raised) root.showSurface(root.lastForm, true);
            if (quitState.observe(next)) return;
            const previousLocalRevision = root.localRevision;
            root.localRevision = next.local_files ? next.local_files.revision : "";
            let libraries = JSON.stringify(next.libraries || []);
            if (libraries !== root.libraryJson) { root.libraryJson = libraries; root.libraryModel = next.libraries || []; }
            if (root.libraryId && !root.libraryModel.some(library => library.id === root.libraryId)) root.libraryId = "";
            if (previousLocalRevision && previousLocalRevision !== root.localRevision || previousAccount !== undefined && previousAccount !== next.account_epoch) {
                Qt.callLater(() => {
                    if (libraryTabs.currentIndex === 4 && root.globalQuery.trim()) root.searchCatalog();
                    else if (root.localSelected) {
                        if (libraryTabs.currentIndex === 0) root.browse(previousTrackOffset);
                        else if (libraryTabs.currentIndex === 2) root.allAlbums();
                        else if (libraryTabs.currentIndex === 3) root.browseArtists("0");
                    }
                });
            }
            let tracks = JSON.stringify(next.page ? next.page.tracks : []);
            if (tracks !== root.trackJson) { root.trackJson = tracks; root.trackModel = next.page ? next.page.tracks : []; }
            let queue = JSON.stringify(next.queue ? next.queue.entries : []);
            if (queue !== root.queueJson) { root.queueJson = queue; root.queueModel = next.queue ? next.queue.entries : []; }
            let lists = JSON.stringify(next.playlists && next.playlists.list ? next.playlists.list.playlists : []);
            if (lists !== root.playlistJson) { root.playlistJson = lists; root.playlistModel = next.playlists && next.playlists.list ? next.playlists.list.playlists : []; }
            let entries = JSON.stringify(next.playlists && next.playlists.selected ? next.playlists.selected.entries : []);
            if (entries !== root.playlistEntriesJson) { root.playlistEntriesJson = entries; root.playlistEntries = next.playlists && next.playlists.selected ? next.playlists.selected.entries : []; }
        }
    }
    Timer { id: searchDelay; interval: 300; onTriggered: root.searchCatalog() }
    FloatingWindow {
        id: window
        title: "Qoda Player"
        visible: root.form === "full"
        implicitWidth: 1200
        implicitHeight: 820
        color: Theme.background
        onClosed: root.hideSurface()

        ThemedControl {
            id: fullFrame
            anchors.fill: parent
            background: Rectangle { color: Theme.background }
        SettingsSheet {
            id: settingsSheet
            palette: fullFrame.palette
            section: root.settingsSection
            width: Math.min(560, window.width - 24)
            height: window.height
            visible: root.settingsSection !== "" && root.form === "full" && window.visible
            onSectionRequested: section => root.settingsSection = section
            onCloseRequested: root.closeSettings()
            onSearchRequested: root.focusSearch()
            onClosed: Qt.callLater(() => {
                if (!settingsSheet.visible && root.form === "full" && window.visible) root.closeSettings();
                root.tryFocusSearch();
            })
            LocalFolders {
                width: parent.width
                visible: root.settingsSection === "folders"
                Layout.fillWidth: true
                snapshot: root.snapshot.local_files || null
                connected: backend.connected
                problem: root.snapshot.error || ""
                onApplyRequested: paths => root.editQueue("set_local_folders", {paths: paths})
                onRescanRequested: root.send("rescan_local_folders")
            }
            Flow {
                width: parent.width
                spacing: 6
                visible: root.accountExpanded
                Button { text: I18n.t("Connect Plex"); enabled: backend.connected && (!root.snapshot.sign_in || !root.snapshot.sign_in.connected); onClicked: root.send("auth_start") }
                Button { text: I18n.t("Disconnect"); enabled: !!root.snapshot.sign_in && root.snapshot.sign_in.connected; ToolTip.visible: hovered; ToolTip.text: I18n.t("Stops playback and removes cached metadata, artwork and the saved session and listening archive. Keeps saved sign-in."); onClicked: root.send("disconnect") }
                Button { text: I18n.t("Restore saved sign-in"); enabled: backend.connected && !!root.snapshot.sign_in && !root.snapshot.sign_in.connected && !root.snapshot.sign_in.keyring_action; onClicked: root.send("restore_signin") }
                Button { text: I18n.t("Reconnect Plex"); visible: !!root.snapshot.sign_in && !!root.snapshot.sign_in.can_reconnect; enabled: backend.connected && !!root.snapshot.sign_in && !root.snapshot.sign_in.connecting && !root.snapshot.sign_in.keyring_action; onClicked: root.editQueue("reconnect", {}) }
                Button { text: I18n.t("Save sign-in"); enabled: backend.connected && !!root.snapshot.sign_in && root.snapshot.sign_in.can_save && !root.snapshot.sign_in.keyring_action; onClicked: root.send("save_signin") }
                Button { text: I18n.t("Sign out & forget"); ToolTip.visible: hovered; ToolTip.text: I18n.t("Stops playback and removes Qoda sign-in, cached metadata/artwork, saved queue and session backups. Keeps general preferences."); enabled: backend.connected && !!root.snapshot.sign_in && root.snapshot.sign_in.keyring_action !== "forget"; onClicked: root.send("forget_signin") }
                Label { text: root.snapshot.sign_in ? I18n.message(root.snapshot.sign_in.message) || "" : ""; color: Theme.muted; width: parent.width; wrapMode: Text.Wrap; textFormat: Text.PlainText }
            }
            AudioSettings {
                visible: root.audioExpanded
                width: parent.width
                snapshot: root.snapshot.replaygain || null
                output: root.snapshot.output || null
                connected: backend.connected
                onModeRequested: mode => root.send("set_replaygain", {mode: mode})
                onOutputRequested: (mode, device, assumeStereo) => root.send("set_output", {mode: mode, device: device, assume_stereo: assumeStereo})
                onRefreshOutputsRequested: root.send("refresh_outputs")
            }
            ColumnLayout {
                visible: root.cacheExpanded
                width: parent.width
                Label { text: root.snapshot.artwork_cache ? (root.snapshot.artwork_cache.usage_known ? I18n.t("%1 · %2 MiB · %3", [I18n.message(root.snapshot.artwork_cache.message), I18n.number(root.snapshot.artwork_cache.bytes / 1048576, 1), I18n.count("%1 cover", "%1 covers", root.snapshot.artwork_cache.entries)]) : I18n.message(root.snapshot.artwork_cache.message)) + (root.snapshot.artwork_cache.stale ? I18n.t(" · Cached artwork may be stale") : "") : I18n.t("Opening artwork cache…"); color: root.snapshot.artwork_cache && root.snapshot.artwork_cache.cleanup_pending ? Theme.danger : Theme.muted; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true }
                Flow {
                    Layout.fillWidth: true
                    Layout.preferredHeight: childrenRect.height
                    spacing: 6
                    Label { text: I18n.t("Disk target (MiB, 0 disables)"); color: Theme.muted; padding: 6 }
                    SpinBox { from: 0; to: 4096; editable: true; value: root.cacheBudgetDraft; onValueModified: root.cacheBudgetDraft = value }
                    Button { text: I18n.t("Apply"); enabled: backend.connected && !root.snapshot.cache_action && !!root.snapshot.artwork_cache && !root.snapshot.artwork_cache.cleanup_pending; onClicked: root.send("set_artwork_budget", {mib: root.cacheBudgetDraft}) }
                    Button { text: root.snapshot.artwork_cache && root.snapshot.artwork_cache.cleanup_pending ? I18n.t("Retry cleanup") : I18n.t("Clear artwork"); enabled: backend.connected && !root.snapshot.cache_action; ToolTip.visible: hovered; ToolTip.text: I18n.t("Keeps your queue, sign-in and cache target. Visible covers are fetched again."); onClicked: root.send("clear_artwork_cache") }
                    Label { visible: !!root.snapshot.cache_action; text: I18n.t("Updating artwork cache…"); color: Theme.muted; padding: 6 }
                }
                Label { text: root.snapshot.metadata_cache ? I18n.t("Metadata · %1", [root.snapshot.metadata_cache.usage_known ? I18n.t("%1 · %2 MiB · %3 · %4", [I18n.message(root.snapshot.metadata_cache.message), I18n.number(root.snapshot.metadata_cache.bytes / 1048576, 1), I18n.count("%1 page", "%1 pages", root.snapshot.metadata_cache.pages), I18n.count("%1 indexed track", "%1 indexed tracks", root.snapshot.metadata_cache.tracks)]) : I18n.message(root.snapshot.metadata_cache.message)]) : I18n.t("Opening metadata cache…"); color: root.snapshot.metadata_cache && root.snapshot.metadata_cache.cleanup_pending ? Theme.danger : Theme.muted; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true }
                Flow {
                    Layout.fillWidth: true
                    Layout.preferredHeight: childrenRect.height
                    spacing: 6
                    Label { text: I18n.t("Metadata target (MiB, 0 disables)"); color: Theme.muted; padding: 6 }
                    SpinBox { from: 0; to: 1024; editable: true; value: root.metadataBudgetDraft; onValueModified: root.metadataBudgetDraft = value }
                    Button { text: I18n.t("Apply metadata target"); enabled: backend.connected && !root.snapshot.metadata_cache_action && !!root.snapshot.metadata_cache && !root.snapshot.metadata_cache.cleanup_pending && root.snapshot.metadata_cache.state !== "opening"; onClicked: root.send("set_metadata_budget", {mib: root.metadataBudgetDraft}) }
                    Button { text: root.snapshot.metadata_cache && root.snapshot.metadata_cache.cleanup_pending ? I18n.t("Retry metadata cleanup") : I18n.t("Clear metadata"); enabled: backend.connected && !root.snapshot.metadata_cache_action; ToolTip.visible: hovered; ToolTip.text: I18n.t("Resets loaded browser views and cached metadata. Keeps your queue, sign-in, artwork and cache targets."); onClicked: root.send("clear_metadata_cache") }
                    Label { visible: !!root.snapshot.metadata_cache_action; text: I18n.t("Updating metadata cache…"); color: Theme.muted; padding: 6 }
                }
            }
            ArchiveBrowser {
                objectName: "listeningArchive"
                visible: root.settingsSection === "history"
                width: parent.width
                snapshot: root.snapshot.archive || null
                connected: backend.connected
                accountEpoch: root.snapshot.account_epoch || ""
                libraries: root.snapshot.libraries || []
                onRequested: (operation, fields) => root.send(operation, fields)
            }
            RowLayout {
                width: parent.width
                visible: root.accountExpanded && !!root.snapshot.signin_url
                Button { text: I18n.t("Open Plex sign-in"); onClicked: { browser.command = ["/usr/bin/brave", root.snapshot.signin_url]; browser.running = true; } }
                TextField { text: root.snapshot.signin_url || ""; readOnly: true; selectByMouse: true; Layout.fillWidth: true }
            }
        }
        Drawer {
            id: queueDrawer
            Shortcut { sequence: "Ctrl+K"; enabled: queueDrawer.visible && !root.reordering; onActivated: root.focusSearch() }
            edge: Qt.RightEdge
            width: Math.min(420, window.width - 24)
            height: window.height
            visible: root.queueRequested && window.width < 1100 && window.visible
            modal: true
            focus: true
            closePolicy: root.reordering ? Popup.NoAutoClose : Popup.CloseOnEscape | Popup.CloseOnPressOutside
            onClosed: Qt.callLater(() => {
                // Closing while the window hides must not mutate visible's
                // dependencies during the same binding evaluation. A reopened
                // drawer also owns the newer request, so leave it untouched.
                if (!queueDrawer.visible) root.queueRequested = false;
                root.tryFocusSearch();
            })
            background: Rectangle { color: Theme.surface; border.color: Theme.border }
        }
        ColumnLayout {
            id: fullLayout
            focus: true
            Keys.priority: Keys.AfterItem
            Keys.onPressed: event => {
                if (root.reordering) {
                    if (event.key === Qt.Key_Escape) { queueReorder.cancel(); playlistReorder.cancel(); }
                    event.accepted = true; return;
                }
                if (event.key === Qt.Key_Space && event.modifiers === Qt.NoModifier && backend.connected && root.snapshot.presentation && (root.snapshot.intent === "playing" ? root.snapshot.presentation.can_pause : root.snapshot.presentation.can_play)) {
                    root.send(root.snapshot.intent === "playing" ? "pause" : "resume"); event.accepted = true;
                }
            }
            Shortcut { sequence: "Ctrl+K"; enabled: window.visible && !root.reordering && !queueDrawer.visible && !settingsSheet.visible && !playerMenu.visible && !libraryPicker.popupVisible; onActivated: root.focusSearch() }
            Shortcut { sequence: "Escape"; enabled: window.visible && root.reordering; onActivated: { queueReorder.cancel(); playlistReorder.cancel(); } }
            anchors.fill: parent
            anchors.margins: 16
            spacing: 8
            RowLayout {
                Layout.fillWidth: true
                Label { text: "QODA"; color: Theme.foreground; font.pixelSize: 20; font.bold: true; font.letterSpacing: 3; Layout.fillWidth: true }
                Button { text: I18n.t("Queue"); visible: window.width < 1100; onClicked: { if (root.queueRequested) root.queueRequested = false; else root.openQueue(queueTabs.currentIndex); } }
                Button { text: I18n.t("Compact"); onClicked: root.showSurface("compact", true) }
                Button { text: I18n.t("Menu"); onClicked: playerMenu.popup() }
                Menu {
                    id: playerMenu
                    Shortcut { sequence: "Ctrl+K"; enabled: playerMenu.visible; onActivated: root.focusSearch() }
                    onClosed: Qt.callLater(root.tryFocusSearch)
                    MenuItem { text: I18n.t("Local music folders"); onTriggered: root.showSettings("folders") }
                    MenuItem { text: I18n.t("Account"); onTriggered: root.showSettings("account") }
                    MenuItem { text: I18n.t("Audio settings"); onTriggered: root.showSettings("audio") }
                    MenuItem { text: I18n.t("Cache settings"); onTriggered: root.showSettings("cache") }
                    MenuItem { text: I18n.t("Listening archive"); onTriggered: root.showSettings("history") }
                    MenuItem { text: I18n.t("Gapless"); checkable: true; checked: !!root.snapshot.gapless && root.snapshot.gapless.enabled; enabled: backend.connected && !!root.snapshot.gapless; onTriggered: root.send("set_gapless", {enabled: checked}) }
                    MenuSeparator {}
                    MenuItem { text: I18n.t("Hide player"); onTriggered: root.hideSurface() }
                    MenuItem { text: I18n.t("Quit player"); onTriggered: quitState.begin() }
                }
            }
            RowLayout {
                visible: !!root.snapshot.output && (!root.snapshot.output.available || !!root.snapshot.output.device_missing) || !!root.snapshot.replaygain && !root.snapshot.replaygain.available || !!root.snapshot.artwork_cache && ["unavailable", "cleanup_pending"].includes(root.snapshot.artwork_cache.state) || !!root.snapshot.metadata_cache && ["unavailable", "cleanup_pending"].includes(root.snapshot.metadata_cache.state)
                Layout.fillWidth: true
                Label { text: I18n.t("Settings need attention"); color: Theme.danger; Layout.fillWidth: true; wrapMode: Text.Wrap }
                Button { text: I18n.t("Review settings"); onClicked: root.showSettings(!!root.snapshot.output && (!root.snapshot.output.available || root.snapshot.output.device_missing) || !!root.snapshot.replaygain && !root.snapshot.replaygain.available ? "audio" : "cache") }
            }
            RowLayout {
                visible: !!root.snapshot.storage && ["unsaved", "blocked"].includes(root.snapshot.storage.state)
                Label { text: root.snapshot.storage ? I18n.message(root.snapshot.storage.message) : I18n.t("Opening session storage…"); color: root.snapshot.storage && (root.snapshot.storage.state === "unsaved" || root.snapshot.storage.state === "blocked") ? Theme.danger : Theme.muted; Layout.fillWidth: true; wrapMode: Text.Wrap; textFormat: Text.PlainText }
                Button { text: I18n.t("Retry save"); enabled: backend.connected; onClicked: root.send("retry_save") }
            }
            DesktopControls {
                Layout.fillWidth: true
                snapshot: root.snapshot.mpris || null
                connected: backend.connected
                onRetryRequested: root.send("retry_desktop")
            }
            Label { visible: !!root.snapshot.gapless && ["review", "fallback"].includes(root.snapshot.gapless.phase); text: root.snapshot.gapless ? I18n.message(root.snapshot.gapless.message) : ""; color: root.snapshot.gapless && root.snapshot.gapless.phase === "review" ? Theme.danger : Theme.muted; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true }
            StreamRecovery {
                Layout.fillWidth: true
                snapshot: root.snapshot.recovery || null
                queue: root.snapshot.queue || null
                connected: backend.connected
                canRetry: root.playbackAvailable
                onActionRequested: (op, fields) => root.send(op, fields)
            }
            Label { visible: !backend.connected || !root.snapshot.sign_in || !root.snapshot.sign_in.connected || !!root.snapshot.error; text: backend.connected ? I18n.message(root.snapshot.status) : I18n.t("Backend disconnected — reconnecting"); color: Theme.muted; textFormat: Text.PlainText; Layout.fillWidth: true; elide: Text.ElideRight }
            Label { visible: !!quitState.error || !!root.snapshot.error; text: I18n.message(quitState.error) || I18n.message(root.snapshot.error) || ""; color: Theme.danger; wrapMode: Text.Wrap; Layout.fillWidth: true; textFormat: Text.PlainText }
                QtObject {
                    id: libraryTabs; property int currentIndex: 2
                    onCurrentIndexChanged: {
                        searchDelay.stop();
                        search.text = currentIndex === 2 ? root.albumQuery : currentIndex === 3 ? root.artistQuery : currentIndex === 4 ? root.globalQuery : root.trackQuery;
                        if (currentIndex === 0 && root.libraryId && (!root.snapshot.page || root.snapshot.page.source !== root.libraryId || root.snapshot.page.query !== root.trackQuery)) root.browse("0");
                        if (currentIndex === 4 && root.globalQuery.trim() && (!root.snapshot.search || root.snapshot.search.query !== root.globalQuery.trim())) root.send("search_all", {query: root.globalQuery});
                        if (currentIndex === 1 && root.libraryId) root.refreshPlaylists();
                        if (currentIndex === 2 && root.libraryId && !root.navigatingArtist && !(root.snapshot.albums && root.snapshot.albums.page && root.snapshot.albums.page.source === root.libraryId && root.snapshot.albums.page.query === root.albumQuery)) root.browseAlbums("0");
                        if (currentIndex === 3 && root.libraryId && !(root.snapshot.albums && root.snapshot.albums.artists && root.snapshot.albums.artists.source === root.libraryId && root.snapshot.albums.artists.query === root.artistQuery)) root.browseArtists("0");
                    }
                }
            RowLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 18
                Rectangle {
                    id: libraryRail
                    Layout.preferredWidth: window.width < 760 ? 122 : 172
                    Layout.fillHeight: true
                    color: Theme.surface
                    radius: 8
                    ScrollView {
                        id: railScroll
                        anchors.fill: parent; anchors.margins: 10
                        contentWidth: availableWidth
                        clip: true
                    ColumnLayout {
                        width: railScroll.availableWidth
                        height: Math.max(implicitHeight, railScroll.availableHeight)
                        spacing: 8
                        Label { text: I18n.t("LIBRARY"); color: Theme.muted; font.pixelSize: 11; font.letterSpacing: 1.5 }
                        LibraryPicker {
                            id: libraryPicker
                            Layout.fillWidth: true
                            Layout.minimumWidth: 0
                            libraries: root.libraryModel
                            selectedId: root.libraryId
                            onSearchRequested: root.focusSearch()
                            onDismissed: Qt.callLater(root.tryFocusSearch)
                            onChosen: source => {
                                searchDelay.stop(); root.libraryId = source;
                                root.playlistSeed = null; root.albumQuery = ""; root.artistQuery = ""; root.trackQuery = "";
                                search.text = libraryTabs.currentIndex === 4 ? root.globalQuery : "";
                                if (libraryTabs.currentIndex === 0) root.browse("0");
                                else if (libraryTabs.currentIndex === 1) root.refreshPlaylists();
                                else if (libraryTabs.currentIndex === 2) root.allAlbums();
                                else if (libraryTabs.currentIndex === 3) root.browseArtists("0");
                            }
                        }
                        Rectangle { Layout.fillWidth: true; implicitHeight: 1; color: Theme.border; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                        Repeater {
                            model: [{title: I18n.t("Albums"), tab: 2}, {title: I18n.t("Artists"), tab: 3}, {title: I18n.t("Tracks"), tab: 0}, {title: I18n.t("Playlists"), tab: 1}, {title: I18n.t("Search"), tab: 4}]
                            delegate: Button {
                                required property var modelData
                                Layout.fillWidth: true
                                text: modelData.title
                                contentItem: Text {
                                    text: parent.text; font: parent.font; color: Theme.foreground
                                    horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter
                                    wrapMode: Text.Wrap
                                }
                                checked: libraryTabs.currentIndex === modelData.tab
                                highlighted: checked
                                onClicked: { root.pendingSearchFocus = false; root.closeSettings(); root.queueRequested = false; libraryTabs.currentIndex = modelData.tab; }
                            }
                        }
                        Item { Layout.fillHeight: true }
                        Label { text: root.snapshot.sign_in && root.snapshot.sign_in.connected ? I18n.t("Plex connected") : root.snapshot.discovery && root.snapshot.discovery.origin === "offline" ? I18n.t("Offline library cache") : I18n.t("Plex disconnected"); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
                        Button { id: railAccount; text: root.snapshot.sign_in && root.snapshot.sign_in.connected ? I18n.t("Account") : I18n.t("Connect Plex"); Layout.fillWidth: true; onClicked: root.showSettings("account") }
                    }
                    }
                }
                ColumnLayout {
                    id: browserPane
                    visible: true
                    Layout.fillWidth: true; Layout.fillHeight: true
                    spacing: 14
                    RowLayout {
                        Layout.fillWidth: true
                        Label { text: [I18n.t("Tracks"), I18n.t("Playlists"), I18n.t("Albums"), I18n.t("Artists"), I18n.t("Search")][libraryTabs.currentIndex]; color: Theme.foreground; font.pixelSize: 26; font.bold: true; Layout.fillWidth: true }
                        TextField { visible: libraryTabs.currentIndex !== 1; id: search; maximumLength: 512; Layout.preferredWidth: Math.min(300, browserPane.width * 0.6); Layout.minimumWidth: 80; placeholderText: libraryTabs.currentIndex === 2 ? I18n.t("Search album titles") : libraryTabs.currentIndex === 3 ? I18n.t("Search artist names") : libraryTabs.currentIndex === 4 ? I18n.t("Search all libraries") : I18n.t("Search this library"); onTextEdited: { root.rememberQuery(); searchDelay.restart(); } onAccepted: { searchDelay.stop(); root.searchCatalog(); } }
                    }
                DiscoveryNotice {
                    Layout.fillWidth: true
                    snapshot: root.snapshot.discovery || null
                    canReconnect: !!root.snapshot.sign_in && !!root.snapshot.sign_in.can_reconnect
                    busy: !backend.connected || !!root.snapshot.sign_in && (!!root.snapshot.sign_in.connecting || !!root.snapshot.sign_in.keyring_action)
                    onReconnectRequested: root.editQueue("reconnect", {})
                    onSignInRequested: root.showSettings("account")
                }
                AlbumBrowser {
                    id: albumBrowser
                    visible: libraryTabs.currentIndex === 2
                    Layout.fillWidth: true; Layout.fillHeight: true
                    snapshot: root.snapshot.albums || null
                    connected: root.playbackAvailable
                    artworkEnabled: window.visible && backend.connected
                    onOpenArtist: reference => root.openArtist(reference)
                    onAllAlbums: root.allAlbums()
                    onOpenAlbum: reference => root.send("album_open", {reference: reference})
                    onPageRequested: direction => root.browseAlbums(root.pageOffset(albumBrowser.page.offset, direction === "previous" ? -100 : 100))
                    onQueueAlbum: (op, fields) => root.editQueue(op, fields)
                    onTrackAction: (op, track) => root.editQueue(op, {source: track.reference.source, key: track.reference.key})
                }
                SearchBrowser {
                    id: globalSearch
                    libraries: root.libraryModel
                    visible: libraryTabs.currentIndex === 4
                    Layout.fillWidth: true; Layout.fillHeight: true
                    snapshot: root.snapshot.search || null
                    query: root.globalQuery
                    connected: backend.connected && root.playbackAvailable
                    onActionRequested: (op, fields) => root.editQueue(op, fields)
                }
                ArtistBrowser {
                    id: artistBrowser
                    visible: libraryTabs.currentIndex === 3
                    Layout.fillWidth: true; Layout.fillHeight: true
                    snapshot: root.snapshot.albums || null
                    onOpenArtist: reference => root.openArtist(reference)
                    onPageRequested: direction => root.browseArtists(root.pageOffset(artistBrowser.page.offset, direction === "previous" ? -100 : 100))
                }
                ColumnLayout {
                    visible: libraryTabs.currentIndex === 0
                    Layout.fillWidth: true
                    Layout.fillHeight: true
            Label { visible: !!root.snapshot.catalog && !!root.snapshot.catalog.cache && root.snapshot.catalog.cache.origin !== "live"; text: root.snapshot.catalog && root.snapshot.catalog.cache ? I18n.message(root.snapshot.catalog.cache.message) : ""; color: Theme.muted; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Label { text: I18n.t("Select tracks with checkboxes or Ctrl/Shift-click titles. The last clicked track seeds a new playlist."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Flow {
                Layout.fillWidth: true
                Layout.preferredHeight: childrenRect.height
                spacing: 6
                Label { text: I18n.t("%1 selected on this page", [I18n.number(trackSelection.count)]); color: Theme.muted; padding: 6 }
                Button { text: I18n.t("Select page"); enabled: root.trackPageReady && root.trackModel.length > 0; onClicked: trackSelection.selectAll() }
                Button { text: I18n.t("Clear selection"); enabled: trackSelection.count > 0; onClicked: trackSelection.clear() }
                Button { text: I18n.t("Play selected"); enabled: root.trackPageReady && trackSelection.count > 0; onClicked: root.bulkTracks("play_selected") }
                Button { text: I18n.t("Play selected next"); enabled: root.trackPageReady && trackSelection.count > 0; onClicked: root.bulkTracks("play_next_selected") }
                Button { text: I18n.t("+ Queue selected"); enabled: root.trackPageReady && trackSelection.count > 0; onClicked: root.bulkTracks("enqueue_selected") }
            }
            Label { visible: !!root.playlistView; text: { if (!root.playlistView) return ""; let library = root.libraryModel.find(item => item.id === root.playlistView.playlist.reference.source); return I18n.t("+ Playlist saves to: %1", [root.playlistView.playlist.title]) + (library ? " · " + library.server : ""); } color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            ListView {
                id: tracks
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                spacing: 5
                model: root.trackModel
                ScrollBar.vertical: ScrollBar {}
                delegate: TrackRow {
                    required property var modelData
                    track: modelData
                    selected: trackSelection.contains(index)
                    enabled: root.trackPageReady
                    playlistEditable: root.playlistWritable
                    width: tracks.width
                    onSelectionRequested: modifiers => { trackSelection.choose(index, modifiers); root.playlistSeed = modelData; }
                    onCheckboxRequested: { trackSelection.toggle(index); root.playlistSeed = modelData; }
                    onActionRequested: op => { if (op === "play") root.send(op, {source: modelData.reference.source, key: modelData.reference.key}); else root.editQueue(op, {source: modelData.reference.source, key: modelData.reference.key}); }
                    onPlaylistRequested: root.playlistEdit({action: "append", track: modelData.reference})
                }
            }
            RowLayout {
                Label { text: root.snapshot.page ? I18n.count("%1 track on this page", "%1 tracks on this page", root.snapshot.page.tracks.length) : I18n.t("Choose a library to browse"); color: Theme.muted; Layout.fillWidth: true }
                Button { text: I18n.t("Previous"); enabled: !!root.snapshot.page && root.snapshot.page.offset !== "0"; onClicked: root.browse(root.pageOffset(root.snapshot.page.offset, -100)) }
                Button { text: I18n.t("Next"); enabled: !!root.snapshot.page && root.snapshot.page.has_more; onClicked: root.browse(root.pageOffset(root.snapshot.page.offset, 100)) }
            }
                }
                ColumnLayout {
                    visible: libraryTabs.currentIndex === 1
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    Label { text: root.localSelected ? I18n.t("Local folders support playback and the queue. Saved playlists are managed on Plex.") : I18n.t("Audio playlists on the selected library's server"); color: Theme.foreground; font.bold: true }
                    Label { text: root.snapshot.playlists ? I18n.message(root.snapshot.playlists.message) : I18n.t("Choose a library, then refresh."); color: root.snapshot.playlists && root.snapshot.playlists.uncertain ? Theme.danger : Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true; textFormat: Text.PlainText }
                    RowLayout {
                        Button { text: I18n.t("Refresh from Plex"); enabled: !root.localSelected && !!root.libraryId && !!root.snapshot.playlists && !root.snapshot.playlists.busy; onClicked: root.refreshPlaylists() }
                        Button { text: I18n.t("Previous lists"); enabled: !!root.snapshot.playlists && !!root.snapshot.playlists.list && root.snapshot.playlists.list.offset !== "0" && !root.snapshot.playlists.busy; onClicked: root.send("playlist_list", {source: root.libraryId, offset: root.pageOffset(root.snapshot.playlists.list.offset, -100)}) }
                        Button { text: I18n.t("More lists"); enabled: !!root.snapshot.playlists && !!root.snapshot.playlists.list && root.snapshot.playlists.list.has_more && !root.snapshot.playlists.busy; onClicked: root.send("playlist_list", {source: root.libraryId, offset: root.pageOffset(root.snapshot.playlists.list.offset, 100)}) }
                    }
                    ComboBox {
                        id: savedLists
                        Layout.fillWidth: true
                        model: root.playlistModel
                        textRole: "title"
                        displayText: root.playlistView ? root.playlistView.playlist.title : I18n.t("Choose a saved playlist")
                        enabled: !!root.snapshot.playlists && !root.snapshot.playlists.busy
                        contentItem: Text { text: savedLists.displayText; textFormat: Text.PlainText; color: Theme.foreground; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
                        delegate: ItemDelegate {
                            id: playlistDelegate
                            required property var modelData
                            width: savedLists.width
                            text: modelData.title + (modelData.smart ? I18n.t(" · Smart / read-only") : "")
                            contentItem: Text { text: playlistDelegate.text; textFormat: Text.PlainText; color: Theme.foreground; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
                        }
                        onActivated: index => root.send("playlist_open", {reference: model[index].reference})
                    }
                    RowLayout {
                        TextField { id: playlistName; placeholderText: I18n.t("New playlist title / rename to…"); maximumLength: 512; Layout.fillWidth: true }
                        Button { text: I18n.t("Create from selected track"); enabled: !root.localSelected && !!root.libraryId && !!root.playlistSeed && playlistName.text.trim().length > 0 && !!root.snapshot.playlists && !root.snapshot.playlists.busy && !root.snapshot.playlists.uncertain; onClicked: root.send("playlist_create", {source: root.libraryId, title: playlistName.text, seed: root.playlistSeed.reference}) }
                    }
                    Label { visible: !!root.playlistSeed; text: root.playlistSeed ? I18n.t("New playlist starts with: %1", [root.playlistSeed.title]) : I18n.t("Select a track in Library to create a playlist."); color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                    Label { text: root.playlistView ? root.playlistView.playlist.title + (root.playlistView.playlist.can_edit && !root.playlistView.playlist.smart ? I18n.t(" · Edits are saved to Plex") : I18n.t(" · Read-only")) : I18n.t("Open a playlist"); color: Theme.foreground; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                    Flow {
                        Layout.fillWidth: true; Layout.preferredHeight: childrenRect.height; spacing: 6
                        Button { text: I18n.t("Play playlist"); enabled: !!root.playlistView && root.playlistEntries.length > 0 && !root.snapshot.playlists.busy; onClicked: root.playlistQueue("playlist_play", null) }
                        Button { text: I18n.t("+ Queue"); enabled: !!root.playlistView && root.playlistEntries.length > 0 && !root.snapshot.playlists.busy; onClicked: root.playlistQueue("playlist_enqueue", null) }
                        Button { text: I18n.t("Rename"); enabled: root.playlistWritable && playlistName.text.trim().length > 0; onClicked: root.playlistEdit({action: "rename", title: playlistName.text}) }
                        Button { text: I18n.t("Delete from Plex"); ToolTip.visible: hovered; ToolTip.text: I18n.t("Deletes this saved playlist from Plex. The local queue is kept."); enabled: root.playlistWritable; onClicked: root.playlistEdit({action: "delete"}) }
                    }
                    ListView {
                        id: playlistTracks
                        highlightFollowsCurrentItem: false
                        highlightRangeMode: ListView.NoHighlightRange
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        model: root.playlistEntries
                        spacing: 4
                        ScrollBar.vertical: ScrollBar {}
                        footer: Item {
                            width: playlistTracks.width; height: playlistReorder.active ? 30 : 0
                            Label { anchors.centerIn: parent; text: I18n.t("Move to end"); color: Theme.muted; visible: playlistReorder.active }
                            ReorderDrop { anchors.fill: parent; controller: playlistReorder; targetId: ""; append: true }
                        }
                        delegate: Rectangle {
                            required property var modelData
                            required property int index
                            width: playlistTracks.width
                            height: width >= 640 ? 72 : 108
                            color: index % 2 ? Theme.alternate : Theme.surface
                            GridLayout {
                                anchors.fill: parent; anchors.margins: 6
                                columns: parent.width >= 640 ? 2 : 1; columnSpacing: 10; rowSpacing: 4
                                RowLayout {
                                    Layout.fillWidth: true
                                    ReorderHandle { controller: playlistReorder; itemId: modelData.id; viewport: playlistTracks }
                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        Label { text: modelData.title; color: Theme.foreground; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                                        Label { text: modelData.track ? modelData.artist : I18n.t("Source unavailable or unresolved"); color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                                    }
                                }
                                RowLayout {
                                    Button { text: I18n.t("Play from here"); enabled: !!root.playlistView && !!modelData.track && !root.snapshot.playlists.busy; onClicked: root.playlistQueue("playlist_play", modelData.id) }
                                    Button { text: "↑"; implicitWidth: 32; leftPadding: 6; rightPadding: 6; enabled: root.playlistWritable && index > 0; onClicked: root.playlistEdit({action: "move", item_id: modelData.id, after: index > 1 ? root.playlistEntries[index - 2].id : null}) }
                                    Button { text: "↓"; implicitWidth: 32; leftPadding: 6; rightPadding: 6; enabled: root.playlistWritable && index + 1 < root.playlistEntries.length; onClicked: root.playlistEdit({action: "move", item_id: modelData.id, after: root.playlistEntries[index + 1].id}) }
                                    Button { text: I18n.t("Remove"); enabled: root.playlistWritable; onClicked: root.playlistEdit({action: "remove", item_id: modelData.id}) }
                                }
                            }
                            ReorderDrop { anchors.fill: parent; controller: playlistReorder; targetId: modelData.id }
                        }
                    }
                }
                }
                Rectangle { visible: window.width >= 1100; width: 1; Layout.fillHeight: true; color: Theme.border }
                Item {
                    id: queueSlot
                    visible: window.width >= 1100
                    Layout.preferredWidth: 350
                    Layout.fillHeight: true
                ColumnLayout {
                    id: queuePane
                    parent: window.width >= 1100 ? queueSlot : queueDrawer.contentItem
                    anchors.fill: parent
                    anchors.margins: window.width >= 1100 ? 0 : 16
                    RowLayout {
                        Layout.fillWidth: true
                        Label { text: I18n.t("Queue · %1", [I18n.number(root.queueModel.length)]); color: Theme.foreground; font.pixelSize: 20; font.bold: true; Layout.fillWidth: true }
                        Button { text: I18n.t("Close"); visible: window.width < 1100; onClicked: root.queueRequested = false }
                    }
                    RowLayout {
                Button { text: I18n.t("Shuffle: %1", [root.snapshot.queue && root.snapshot.queue.shuffled ? I18n.t("On") : I18n.t("Off")]); enabled: !!root.snapshot.queue; onClicked: root.editQueue("set_shuffle", {enabled: !root.snapshot.queue.shuffled}) }
                Button { text: I18n.t("Repeat: %1", [I18n.state(root.snapshot.queue ? root.snapshot.queue.repeat : "off")]); enabled: !!root.snapshot.queue; onClicked: { let modes = ["off", "one", "all"]; root.editQueue("set_repeat", {mode: modes[(modes.indexOf(root.snapshot.queue.repeat) + 1) % 3]}); } }
                    }
                    TabBar {
                        id: queueTabs
                        Layout.fillWidth: true
                        TabButton { text: I18n.t("Queue") }
                        TabButton { text: I18n.t("Previous path") }
                        TabButton { text: I18n.t("Unavailable") + (root.snapshot.queue && root.snapshot.queue.unavailable_entries && root.snapshot.queue.unavailable_entries.length ? " (" + root.snapshot.queue.unavailable_entries.length + ")" : "") }
                    }
                    Label { visible: queueTabs.currentIndex === 0; text: root.snapshot.queue && root.snapshot.queue.navigating ? I18n.t("Previous navigation · Next returns forward") : root.snapshot.queue && root.snapshot.queue.ended ? I18n.t("End of selection · Play pending or Replay") : I18n.t("Selected and upcoming"); color: Theme.muted; textFormat: Text.PlainText }
                    ListView {
                        id: queueView
                        highlightFollowsCurrentItem: false
                        highlightRangeMode: ListView.NoHighlightRange
                        visible: queueTabs.currentIndex === 0
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        model: root.queueModel
                        clip: true
                        spacing: 4
                        ScrollBar.vertical: ScrollBar {}
                        footer: Item {
                            width: queueView.width; height: queueReorder.active ? 30 : 0
                            Label { anchors.centerIn: parent; text: I18n.t("Move to end"); color: Theme.muted; visible: queueReorder.active }
                            ReorderDrop { anchors.fill: parent; controller: queueReorder; targetId: ""; append: true }
                        }
                        delegate: Rectangle {
                            required property var modelData
                            required property int index
                            width: queueView.width
                            height: Math.max(modelData.unavailable ? 110 : 88, queueRowBody.implicitHeight + 16)
                            radius: 5
                            color: modelData.selected ? Theme.selected : Theme.surface
                            Rectangle { width: 2; anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom; anchors.margins: 6; color: Theme.accent; visible: modelData.selected; radius: 1 }
                            ColumnLayout {
                                id: queueRowBody
                                anchors.fill: parent
                                anchors.margins: 8
                                spacing: 3
                                RowLayout {
                                    Layout.fillWidth: true
                                    ReorderHandle { controller: queueReorder; itemId: modelData.id; viewport: queueView; movable: !!modelData.can_move }
                                    Label { text: modelData.track.title; color: Theme.foreground; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                                    Label { visible: modelData.selected; text: I18n.t("Selected"); color: Theme.accent; font.pixelSize: 10 }
                                }
                                Label { text: modelData.track.artist + (modelData.track.album ? " · " + modelData.track.album : ""); color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                                Label { visible: !!modelData.unavailable; text: modelData.unavailable ? I18n.t("Unavailable: %1", [I18n.state(modelData.unavailable)]) : ""; color: Theme.danger; textFormat: Text.PlainText; Layout.fillWidth: true }
                                RowLayout {
                                    Button { text: modelData.unavailable ? I18n.t("Retry") : I18n.t("Play"); enabled: root.playbackAvailable; onClicked: root.editQueue("queue_play", {occurrence: modelData.id}) }
                                    Button { text: "↑"; implicitWidth: 32; leftPadding: 6; rightPadding: 6; enabled: modelData.can_move && index > 0 && root.queueModel[index - 1].can_move; onClicked: root.moveEntry(index, -1) }
                                    Button { text: "↓"; implicitWidth: 32; leftPadding: 6; rightPadding: 6; enabled: modelData.can_move && index + 1 < root.queueModel.length && root.queueModel[index + 1].can_move; onClicked: root.moveEntry(index, 1) }
                                    Button { text: I18n.t("Remove"); onClicked: root.editQueue("queue_remove", {occurrence: modelData.id}) }
                                }
                            }
                            ReorderDrop { anchors.fill: parent; controller: queueReorder; targetId: modelData.id; movable: !!modelData.can_move }
                        }
                    }
                    NavigationBrowser {
                        id: navigationBrowser
                        visible: queueTabs.currentIndex === 1
                        Layout.fillWidth: true; Layout.fillHeight: true
                        snapshot: root.snapshot.navigation || null
                        connected: backend.connected
                        onPageRequested: (offset, fence) => { fence.offset = offset; root.send("navigation_page", fence); }
                        onRevisitRequested: fields => root.send("navigation_select", fields)
                    }
                    UnavailableBrowser {
                        id: unavailableBrowser
                        visible: queueTabs.currentIndex === 2
                        Layout.fillWidth: true; Layout.fillHeight: true
                        snapshot: root.snapshot.queue || null
                        connected: backend.connected
                        canRetry: root.playbackAvailable
                        onActionRequested: (op, fields) => root.send(op, fields)
                    }
                    RowLayout {
                        Button { text: I18n.t("Clear upcoming"); onClicked: root.editQueue("clear_upcoming") }
                        Button { text: I18n.t("Stop & clear"); onClicked: root.editQueue("stop_clear") }
                    }
                    Label { visible: !!root.snapshot.queue && root.snapshot.queue.navigation_dropped !== "0"; text: I18n.t("Previous retains the latest 4096 nominal visits; earlier visits were omitted."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
                    Label { text: I18n.t("Your queue restores paused."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
                }
                }
            }
            PlayerTransport {
                Layout.fillWidth: true
                snapshot: root.snapshot
                selectedTrack: root.selectedTrack
                connected: backend.connected
                onTransportRequested: op => root.send(op)
                onQueueRequested: op => root.editQueue(op)
                onSeekRequested: seconds => root.send("seek", {seconds: seconds})
            }
        }
        }
    }
    CompactPlayer {
        id: compact
        notice: quitState.error
        snapshot: root.snapshot
        selectedTrack: root.selectedTrack
        connected: backend.connected
        onSearchRequested: root.focusSearch()
        onExpandRequested: root.showSurface("full", true)
        onHideRequested: root.hideSurface()
        onQuitRequested: quitState.begin()
        onTransportRequested: op => root.send(op)
        onQueueRequested: op => { if (op === "show") { root.openQueue(0); } else root.editQueue(op); }
    }
    IpcHandler {
        target: "control"
        function show(): void { root.showSurface(root.lastForm, true); }
        function showFull(): void { root.showSurface("full", true); }
        function showCompact(): void { root.showSurface("compact", true); }
        function previewCompact(): void { root.showSurface("compact", false); }
        function toggle(): void { root.toggleSurface(); }
        function dismiss(): void { if (root.form === "compact") root.hideSurface(); }
        function hide(): void { root.hideSurface(); }
        function retryDesktopControls(): void { root.send("retry_desktop"); }
        function showSearch(): void { root.focusSearch(); }
        function showAccount(): void { root.showSettings("account"); }
        function showAudio(): void { root.showSettings("audio"); }
        function showCache(): void { root.showSettings("cache"); }
        function browseTracks(): void { root.pendingSearchFocus = false; root.closeSettings(); root.queueRequested = false; libraryTabs.currentIndex = 0; root.showSurface("full", true); }
        function browseAlbums(): void { root.pendingSearchFocus = false; root.closeSettings(); root.queueRequested = false; albumBrowser.showDetails = !!albumBrowser.selected; libraryTabs.currentIndex = 2; root.showSurface("full", true); }
        function albumOverview(): void { root.pendingSearchFocus = false; root.closeSettings(); root.queueRequested = false; albumBrowser.showDetails = false; libraryTabs.currentIndex = 2; root.showSurface("full", true); }
        function showUnavailable(): void { root.openQueue(2); }
        function showNavigation(): void { root.openQueue(1); }
        function showQueue(): void { root.openQueue(0); }
        function browseArtists(): void { root.pendingSearchFocus = false; root.closeSettings(); root.queueRequested = false; libraryTabs.currentIndex = 3; root.showSurface("full", true); }
        function browsePlaylists(): void { root.pendingSearchFocus = false; root.closeSettings(); root.queueRequested = false; libraryTabs.currentIndex = 1; root.showSurface("full", true); }
        function status(): string { return JSON.stringify({localization: {language: I18n.language, preferences: I18n.preferences, formatLocale: Qt.locale().name}, discovery: root.snapshot.discovery || null, theme: {system: Theme.system, background: String(Theme.background), foreground: String(Theme.foreground), accent: String(Theme.accent), controlText: String(fullFrame.palette.text), controlHighlight: String(fullFrame.palette.highlight)}, libraryPicker: {source: root.libraryId, label: libraryPicker.label(libraryPicker.selected), open: libraryPicker.popupVisible}, settings: {section: root.settingsSection, visible: settingsSheet.visible, scrollable: settingsSheet.scrollable}, searchFocused: search.activeFocus, reorder: {queueActive: queueReorder.active, playlistActive: playlistReorder.active, queueEnabled: queueReorder.enabled, playlistEnabled: playlistReorder.enabled}, search: root.snapshot.search || null, output: root.snapshot.output || null, replaygain: root.snapshot.replaygain || null, audioExpanded: root.audioExpanded, uiCapability: root.snapshot.ui || null, unavailable: {visible: queueTabs.currentIndex === 2, canRetry: unavailableBrowser.canRetry, entries: unavailableBrowser.rows.length}, desktopControls: root.snapshot.mpris || null, navigation: {visible: queueTabs.currentIndex === 1, entries: navigationBrowser.rows.length, viewportHeight: navigationBrowser.viewportHeight, page: root.snapshot.navigation || null}, artists: artistBrowser.artistModel.length, queueDrawerOpen: queueDrawer.opened, queueDrawerVisible: queueDrawer.visible, albums: {canQueue: albumBrowser.canQueue, cache: root.snapshot.albums ? root.snapshot.albums.cache || null : null, thumbnailsReady: albumBrowser.readyThumbnails, artistContext: !!albumBrowser.page && !!albumBrowser.page.artist,details: albumBrowser.showDetails, count: albumBrowser.albumModel.length, tracks: albumBrowser.trackModel.length, coverReady: albumBrowser.coverReady, trackViewport: albumBrowser.trackViewport}, geometry: {width: window.width, height: window.height, playlistHeight: playlistTracks.height, sidebarWidth: libraryRail.width, railHeight: railScroll.height, railContentHeight: railScroll.contentHeight, accountTop: railAccount.mapToItem(railScroll, 0, 0).y, accountBottom: railAccount.mapToItem(railScroll, 0, railAccount.height).y, browserWidth: browserPane.width, browserHeight: browserPane.height, queueVisible: queueSlot.visible || queueDrawer.opened, queueWidth: queuePane.width, queueHeight: queuePane.height, queueRowHeight: queueView.itemAtIndex(0)?.height || 0}, connected: backend.connected, instance: root.instance, playlists: root.snapshot.playlists ? {busy: root.snapshot.playlists.busy, uncertain: root.snapshot.playlists.uncertain, lists: root.playlistModel.length, entries: root.playlistEntries.length, order: root.playlistEntries.map(row => row.id)} : null, storage: root.snapshot.storage || null, queue: root.snapshot.queue ? {shuffled: root.snapshot.queue.shuffled, repeat: root.snapshot.queue.repeat, previousAvailable: root.snapshot.queue.previous_available, position: root.snapshot.queue.position_seconds, session: root.snapshot.queue.session, revision: root.snapshot.queue.revision, entries: root.snapshot.queue.entries.map(row => ({id: row.id, selected: row.selected, key: row.track.reference.key})), ended: root.snapshot.queue.ended} : null, signIn: root.snapshot.sign_in || null, libraries: (root.snapshot.libraries || []).length, tracks: root.snapshot.page ? root.snapshot.page.tracks.length : 0, selectedTracks: trackSelection.count, trackPageReady: root.trackPageReady, visible: window.visible || compact.visible, form: root.form, lastForm: root.lastForm, compact: {visible: compact.visible, mapped: compact.backingWindowVisible, width: compact.width, height: compact.height, screen: compact.screen ? compact.screen.name : null, keyboardMode: compact.keyboardMode}, presentation: root.snapshot.presentation || null, artworkCache: root.snapshot.artwork_cache || null, metadataCache: root.snapshot.metadata_cache || null, catalogCache: root.snapshot.catalog ? root.snapshot.catalog.cache || null : null, cacheExpanded: root.cacheExpanded, query: search.text, tab: libraryTabs.currentIndex}); }
        function quit(): void { quitState.begin(); }
    }
}
