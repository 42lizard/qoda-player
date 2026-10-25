//@ pragma AppId qoda-player
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io

ShellRoot {
    id: root
    property var snapshot: ({albums: {page: null, selected: null, busy: false, message: "Choose a library to browse albums."}, libraries: [], status: "Connecting backend", intent: "paused", playlists: {list: null, selected: null, busy: false, uncertain: false, message: ""}})
    property var libraryModel: []
    property var trackModel: []
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
    property bool accountExpanded: false
    property bool queueRequested: false
    readonly property bool queueOnly: window.width < 980 && queueRequested
    property string playlistJson: "[]"
    property string playlistEntriesJson: "[]"
    property var playlistView: snapshot.playlists ? snapshot.playlists.selected : null
    function playlistEdit(edit) {
        if (playlistView) send("playlist_edit", {reference: playlistView.playlist.reference, expected_revision: playlistView.revision, edit: edit});
    }
    function playlistQueue(op, item) {
        if (!playlistView) return;
        let fields = {reference: playlistView.playlist.reference, expected_revision: playlistView.revision};
        if (item) fields.item_id = item;
        editQueue(op, fields);
    }
    function refreshPlaylists() { if (libraryId) send("playlist_refresh", {source: libraryId}); }

    property string libraryId: ""
    property string instance: ""
    property int serial: 0
    property string nonce: Date.now().toString(36)
    function send(op, fields) {
        if (!socket.connected || !instance) return;
        let data = fields || {};
        data.op = op;
        data.instance = instance;
        data.account_epoch = snapshot.account_epoch || "0";
        data.id = nonce + "-" + (++serial);
        socket.write(JSON.stringify(data) + "\n");
        socket.flush();
    }
    function editQueue(op, fields) {
        if (!snapshot.queue) return;
        let data = fields || {};
        data.queue_session = snapshot.queue.session;
        data.queue_revision = snapshot.queue.revision;
        send(op, data);
    }
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
    function rememberQuery() { if (libraryTabs.currentIndex === 2) albumQuery = search.text; else if (libraryTabs.currentIndex === 3) artistQuery = search.text; else trackQuery = search.text; }
    function searchCatalog() { rememberQuery(); if (libraryTabs.currentIndex === 2) browseAlbums("0"); else if (libraryTabs.currentIndex === 3) browseArtists("0"); else browse("0"); }
    function browse(offset) {
        if (libraryId) send("browse", {source: libraryId, offset: offset || "0", query: search.text});
    }
    Process { id: browser }
    Socket {
        id: socket
        path: Quickshell.env("XDG_RUNTIME_DIR") + "/qoda/backend.sock"
        connected: true
        onConnectedChanged: {
            if (connected) { root.instance = ""; write('{"op":"hello","protocol":1}\n'); flush(); }
        }
        parser: SplitParser {
            onRead: data => {
                try {
                    let next = JSON.parse(data);
                    if (next.protocol !== 1 || !next.instance) return;
                    if (root.instance === next.instance) {
                        let a = next.revision, b = root.snapshot.revision || "0";
                        if (a.length < b.length || (a.length === b.length && a < b)) return;
                    }
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
                    root.instance = next.instance;
                    root.snapshot = next;
                    let libraries = JSON.stringify(next.libraries || []);
                    if (libraries !== root.libraryJson) { root.libraryJson = libraries; root.libraryModel = next.libraries || []; }
                    let tracks = JSON.stringify(next.page ? next.page.tracks : []);
                    if (tracks !== root.trackJson) { root.trackJson = tracks; root.trackModel = next.page ? next.page.tracks : []; }
                    let queue = JSON.stringify(next.queue ? next.queue.entries : []);
                    if (queue !== root.queueJson) { root.queueJson = queue; root.queueModel = next.queue ? next.queue.entries : []; }
                    let lists = JSON.stringify(next.playlists && next.playlists.list ? next.playlists.list.playlists : []);
                    if (lists !== root.playlistJson) { root.playlistJson = lists; root.playlistModel = next.playlists && next.playlists.list ? next.playlists.list.playlists : []; }
                    let entries = JSON.stringify(next.playlists && next.playlists.selected ? next.playlists.selected.entries : []);
                    if (entries !== root.playlistEntriesJson) { root.playlistEntriesJson = entries; root.playlistEntries = next.playlists && next.playlists.selected ? next.playlists.selected.entries : []; }
                } catch (e) { }
            }
        }
    }
    Timer { interval: 1000; repeat: true; running: !socket.connected; onTriggered: socket.connected = true }
    Timer { id: searchDelay; interval: 300; onTriggered: root.searchCatalog() }
    FloatingWindow {
        id: window
        title: "Qoda Player"
        visible: true
        implicitWidth: 1200
        implicitHeight: 820
        color: "#141820"
        onClosed: visible = false

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 16
            spacing: 8
            Flow {
                Layout.fillWidth: true
                Layout.preferredHeight: childrenRect.height
                spacing: 6
                Label { text: "Qoda"; color: "#f2e9db"; font.pixelSize: 28; font.bold: true }
                Label { text: "Plex music"; color: "#acb8c8" }
                Button { text: root.queueOnly ? "Browser" : "Queue"; visible: window.width < 980; onClicked: root.queueRequested = !root.queueRequested }
                Button { text: "Account"; onClicked: root.accountExpanded = !root.accountExpanded }
                Button { text: "Connect Plex"; enabled: socket.connected && (!root.snapshot.sign_in || !root.snapshot.sign_in.connected); onClicked: root.send("auth_start") }
                Button { text: "Disconnect"; ToolTip.visible: hovered; ToolTip.text: "Stops playback and removes the saved queue and session backups. Keeps saved sign-in."; onClicked: root.send("disconnect") }
                Button { text: "Hide"; onClicked: window.visible = false }
                Button { text: "Quit"; onClicked: { root.send("quit"); Qt.quit(); } }
            }
            Flow {
                Layout.fillWidth: true
                Layout.preferredHeight: childrenRect.height
                spacing: 6
                visible: root.accountExpanded || !root.snapshot.sign_in || !root.snapshot.sign_in.connected || root.snapshot.sign_in.saved !== "present"
                Button { text: "Restore saved sign-in"; enabled: socket.connected && !!root.snapshot.sign_in && !root.snapshot.sign_in.connected && !root.snapshot.sign_in.keyring_action; onClicked: root.send("restore_signin") }
                Button { text: "Save sign-in"; enabled: socket.connected && !!root.snapshot.sign_in && root.snapshot.sign_in.can_save && !root.snapshot.sign_in.keyring_action; onClicked: root.send("save_signin") }
                Button { text: "Sign out & forget"; ToolTip.visible: hovered; ToolTip.text: "Stops playback and removes Qoda sign-in, saved queue and session backups. Keeps general preferences."; enabled: socket.connected && !!root.snapshot.sign_in && root.snapshot.sign_in.keyring_action !== "forget"; onClicked: root.send("forget_signin") }
                Label { text: root.snapshot.sign_in ? root.snapshot.sign_in.message || "" : ""; color: "#acb8c8"; width: parent.width; wrapMode: Text.Wrap; textFormat: Text.PlainText }
            }
            RowLayout {
                Label { text: root.snapshot.storage ? root.snapshot.storage.message : "Opening session storage…"; color: root.snapshot.storage && (root.snapshot.storage.state === "unsaved" || root.snapshot.storage.state === "blocked") ? "#f2a89b" : "#acb8c8"; Layout.fillWidth: true; wrapMode: Text.Wrap; textFormat: Text.PlainText }
                Button { text: "Retry save"; enabled: socket.connected; onClicked: root.send("retry_save") }
            }
            Label { text: socket.connected ? root.snapshot.status : "Backend disconnected — reconnecting"; color: "#acb8c8"; textFormat: Text.PlainText; Layout.fillWidth: true; elide: Text.ElideRight }
            Label { visible: !!root.snapshot.error; text: root.snapshot.error || ""; color: "#f2a89b"; wrapMode: Text.Wrap; Layout.fillWidth: true; textFormat: Text.PlainText }
            RowLayout {
                visible: !!root.snapshot.signin_url
                Button { text: "Open Plex sign-in"; onClicked: { browser.command = ["/usr/bin/brave", root.snapshot.signin_url]; browser.running = true; } }
                TextField { text: root.snapshot.signin_url || ""; readOnly: true; selectByMouse: true; Layout.fillWidth: true }
            }
            GridLayout {
                Layout.fillWidth: true
                columns: window.width < 980 ? 2 : 3
                ComboBox {
                    currentIndex: root.libraryModel.findIndex(library => library.id === root.libraryId)
                    Layout.preferredWidth: 220
                    Layout.minimumWidth: 0
                    Layout.fillWidth: true
                    model: root.libraryModel
                    textRole: "title"
                    onActivated: index => { searchDelay.stop(); root.libraryId = model[index].id; root.playlistSeed = null; root.albumQuery = ""; root.artistQuery = ""; root.trackQuery = ""; search.text = ""; root.browse("0"); if (libraryTabs.currentIndex === 1) root.refreshPlaylists(); if (libraryTabs.currentIndex === 2) root.allAlbums(); if (libraryTabs.currentIndex === 3) root.browseArtists("0"); }
                    displayText: root.libraryId ? currentText : "Choose a music library"
                }
                TabBar {
                    id: libraryTabs; currentIndex: 2; Layout.preferredWidth: 320
                    TabButton { text: "Tracks" } TabButton { text: "Playlists" } TabButton { text: "Albums" } TabButton { text: "Artists" }
                    onCurrentIndexChanged: {
                        searchDelay.stop();
                        search.text = currentIndex === 2 ? root.albumQuery : currentIndex === 3 ? root.artistQuery : root.trackQuery;
                        if (currentIndex === 1 && root.libraryId) root.refreshPlaylists();
                        if (currentIndex === 2 && root.libraryId && !root.navigatingArtist && !(root.snapshot.albums && root.snapshot.albums.page && root.snapshot.albums.page.source === root.libraryId && root.snapshot.albums.page.query === root.albumQuery)) root.browseAlbums("0");
                        if (currentIndex === 3 && root.libraryId && !(root.snapshot.albums && root.snapshot.albums.artists && root.snapshot.albums.artists.source === root.libraryId && root.snapshot.albums.artists.query === root.artistQuery)) root.browseArtists("0");
                    }
                }
                TextField { visible: libraryTabs.currentIndex !== 1; id: search; Layout.fillWidth: true; Layout.columnSpan: window.width < 980 ? 2 : 1; placeholderText: libraryTabs.currentIndex === 2 ? "Search album titles" : libraryTabs.currentIndex === 3 ? "Search artist names" : "Search this library"; onTextEdited: { root.rememberQuery(); searchDelay.restart(); } onAccepted: { searchDelay.stop(); root.searchCatalog(); } }
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 16
                AlbumBrowser {
                    id: albumBrowser
                    visible: libraryTabs.currentIndex === 2 && !root.queueOnly
                    Layout.fillWidth: true; Layout.fillHeight: true
                    snapshot: root.snapshot.albums || null
                    connected: !!root.snapshot.sign_in && root.snapshot.sign_in.connected
                    artworkEnabled: window.visible && socket.connected
                    onOpenArtist: reference => root.openArtist(reference)
                    onAllAlbums: root.allAlbums()
                    onOpenAlbum: reference => root.send("album_open", {reference: reference})
                    onPageRequested: direction => root.browseAlbums(root.pageOffset(albumBrowser.page.offset, direction === "previous" ? -100 : 100))
                    onQueueAlbum: (op, fields) => root.editQueue(op, fields)
                    onTrackAction: (op, track) => root.editQueue(op, {source: track.reference.source, key: track.reference.key})
                }
                ArtistBrowser {
                    id: artistBrowser
                    visible: libraryTabs.currentIndex === 3 && !root.queueOnly
                    Layout.fillWidth: true; Layout.fillHeight: true
                    snapshot: root.snapshot.albums || null
                    onOpenArtist: reference => root.openArtist(reference)
                    onPageRequested: direction => root.browseArtists(root.pageOffset(artistBrowser.page.offset, direction === "previous" ? -100 : 100))
                }
                ColumnLayout {
                    visible: libraryTabs.currentIndex === 0 && !root.queueOnly
                    Layout.fillWidth: true
                    Layout.fillHeight: true
            Label { text: "Click a track title to select the first track for a new playlist."; color: "#acb8c8"; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Label { visible: !!root.playlistView; text: { if (!root.playlistView) return ""; let library = root.libraryModel.find(item => item.id === root.playlistView.playlist.reference.source); return "+ Playlist saves to: " + root.playlistView.playlist.title + (library ? " · " + library.server : ""); } color: "#acb8c8"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            ListView {
                id: tracks
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                spacing: 5
                model: root.trackModel
                ScrollBar.vertical: ScrollBar {}
                delegate: Rectangle {
                    required property var modelData
                    required property int index
                    width: tracks.width
                    height: 68
                    color: root.playlistSeed && root.playlistSeed.reference.source === modelData.reference.source && root.playlistSeed.reference.key === modelData.reference.key ? "#344354" : index % 2 ? "#1a202a" : "#202735"
                    radius: 5
                    TapHandler { onTapped: root.playlistSeed = modelData }
                    RowLayout {
                        anchors.fill: parent
                        anchors.margins: 10
                        ColumnLayout {
                            Layout.fillWidth: true
                            Label { text: modelData.title; color: "#f2e9db"; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                            Label { text: modelData.artist + " · " + modelData.album; color: "#acb8c8"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                        }
                        Button { text: "Play now"; onClicked: root.send("play", {source: modelData.reference.source, key: modelData.reference.key}) }
                        Button { text: "Next"; onClicked: root.editQueue("play_next", {source: modelData.reference.source, key: modelData.reference.key}) }
                        Button { text: "+ Playlist"; enabled: !!root.playlistView && root.playlistView.playlist.can_edit && !root.snapshot.playlists.busy && !root.snapshot.playlists.uncertain; onClicked: root.playlistEdit({action: "append", track: modelData.reference}) }
                        Button { text: "+ Queue"; onClicked: root.editQueue("enqueue", {source: modelData.reference.source, key: modelData.reference.key}) }
                    }
                }
            }
            RowLayout {
                Label { text: root.snapshot.page ? root.snapshot.page.tracks.length + " tracks on this page" : "Choose a library to browse"; color: "#acb8c8"; Layout.fillWidth: true }
                Button { text: "Previous"; enabled: !!root.snapshot.page && root.snapshot.page.offset !== "0"; onClicked: root.browse(root.pageOffset(root.snapshot.page.offset, -100)) }
                Button { text: "Next"; enabled: !!root.snapshot.page && root.snapshot.page.has_more; onClicked: root.browse(root.pageOffset(root.snapshot.page.offset, 100)) }
            }
                }
                ColumnLayout {
                    visible: libraryTabs.currentIndex === 1 && !root.queueOnly
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    Label { text: "Audio playlists on the selected library's server"; color: "#f2e9db"; font.bold: true }
                    Label { text: root.snapshot.playlists ? root.snapshot.playlists.message : "Choose a library, then refresh."; color: root.snapshot.playlists && root.snapshot.playlists.uncertain ? "#f2a89b" : "#acb8c8"; wrapMode: Text.Wrap; Layout.fillWidth: true; textFormat: Text.PlainText }
                    RowLayout {
                        Button { text: "Refresh from Plex"; enabled: !!root.libraryId && !!root.snapshot.playlists && !root.snapshot.playlists.busy; onClicked: root.refreshPlaylists() }
                        Button { text: "Previous lists"; enabled: !!root.snapshot.playlists && !!root.snapshot.playlists.list && root.snapshot.playlists.list.offset !== "0" && !root.snapshot.playlists.busy; onClicked: root.send("playlist_list", {source: root.libraryId, offset: root.pageOffset(root.snapshot.playlists.list.offset, -100)}) }
                        Button { text: "More lists"; enabled: !!root.snapshot.playlists && !!root.snapshot.playlists.list && root.snapshot.playlists.list.has_more && !root.snapshot.playlists.busy; onClicked: root.send("playlist_list", {source: root.libraryId, offset: root.pageOffset(root.snapshot.playlists.list.offset, 100)}) }
                    }
                    ComboBox {
                        id: savedLists
                        Layout.fillWidth: true
                        model: root.playlistModel
                        textRole: "title"
                        displayText: root.playlistView ? root.playlistView.playlist.title : "Choose a saved playlist"
                        enabled: !!root.snapshot.playlists && !root.snapshot.playlists.busy
                        contentItem: Text { text: savedLists.displayText; textFormat: Text.PlainText; color: "#f2e9db"; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
                        delegate: ItemDelegate {
                            id: playlistDelegate
                            required property var modelData
                            width: savedLists.width
                            text: modelData.title + (modelData.smart ? " · Smart / read-only" : "")
                            contentItem: Text { text: playlistDelegate.text; textFormat: Text.PlainText; color: "#f2e9db"; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
                        }
                        onActivated: index => root.send("playlist_open", {reference: model[index].reference})
                    }
                    RowLayout {
                        TextField { id: playlistName; placeholderText: "New playlist title / rename to…"; maximumLength: 512; Layout.fillWidth: true }
                        Button { text: "Create from selected track"; enabled: !!root.libraryId && !!root.playlistSeed && playlistName.text.trim().length > 0 && !!root.snapshot.playlists && !root.snapshot.playlists.busy && !root.snapshot.playlists.uncertain; onClicked: root.send("playlist_create", {source: root.libraryId, title: playlistName.text, seed: root.playlistSeed.reference}) }
                    }
                    Label { visible: !!root.playlistSeed; text: root.playlistSeed ? "New playlist starts with: " + root.playlistSeed.title : "Select a track in Library to create a playlist."; color: "#acb8c8"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                    Label { text: root.playlistView ? root.playlistView.playlist.title + (root.playlistView.playlist.can_edit ? " · Edits are saved to Plex" : " · Read-only") : "Open a playlist"; color: "#f2e9db"; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                    RowLayout {
                        Button { text: "Play playlist"; enabled: !!root.playlistView && root.playlistEntries.length > 0 && !root.snapshot.playlists.busy; onClicked: root.playlistQueue("playlist_play", null) }
                        Button { text: "+ Queue"; enabled: !!root.playlistView && root.playlistEntries.length > 0 && !root.snapshot.playlists.busy; onClicked: root.playlistQueue("playlist_enqueue", null) }
                        Button { text: "Rename"; enabled: !!root.playlistView && root.playlistView.playlist.can_edit && !root.snapshot.playlists.busy && !root.snapshot.playlists.uncertain && playlistName.text.trim().length > 0; onClicked: root.playlistEdit({action: "rename", title: playlistName.text}) }
                        Button { text: "Delete from Plex"; ToolTip.visible: hovered; ToolTip.text: "Deletes this saved playlist from Plex. The local queue is kept."; enabled: !!root.playlistView && root.playlistView.playlist.can_edit && !root.snapshot.playlists.busy && !root.snapshot.playlists.uncertain; onClicked: root.playlistEdit({action: "delete"}) }
                    }
                    ListView {
                        id: playlistTracks
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        model: root.playlistEntries
                        spacing: 4
                        ScrollBar.vertical: ScrollBar {}
                        delegate: Rectangle {
                            required property var modelData
                            required property int index
                            width: playlistTracks.width
                            height: 62
                            color: index % 2 ? "#1a202a" : "#202735"
                            RowLayout {
                                anchors.fill: parent
                                anchors.margins: 6
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    Label { text: modelData.title; color: "#f2e9db"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                                    Label { text: modelData.track ? modelData.artist : "Source unavailable or unresolved"; color: "#acb8c8"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                                }
                                Button { text: "Play from here"; enabled: !!root.playlistView && !!modelData.track && !root.snapshot.playlists.busy; onClicked: root.playlistQueue("playlist_play", modelData.id) }
                                Button { text: "↑"; enabled: !!root.playlistView && root.playlistView.playlist.can_edit && !root.snapshot.playlists.busy && !root.snapshot.playlists.uncertain && index > 0; onClicked: root.playlistEdit({action: "move", item_id: modelData.id, after: index > 1 ? root.playlistEntries[index - 2].id : null}) }
                                Button { text: "↓"; enabled: !!root.playlistView && root.playlistView.playlist.can_edit && !root.snapshot.playlists.busy && !root.snapshot.playlists.uncertain && index + 1 < root.playlistEntries.length; onClicked: root.playlistEdit({action: "move", item_id: modelData.id, after: root.playlistEntries[index + 1].id}) }
                                Button { text: "Remove"; enabled: !!root.playlistView && root.playlistView.playlist.can_edit && !root.snapshot.playlists.busy && !root.snapshot.playlists.uncertain; onClicked: root.playlistEdit({action: "remove", item_id: modelData.id}) }
                            }
                        }
                    }
                }
                Rectangle { visible: window.width >= 980; width: 1; Layout.fillHeight: true; color: "#394355" }
                ColumnLayout {
                    visible: window.width >= 980 || root.queueOnly
                    Layout.fillWidth: root.queueOnly
                    Layout.preferredWidth: 340
                    Layout.fillHeight: true
                    Label { text: "Queue · " + root.queueModel.length; color: "#f2e9db"; font.pixelSize: 20; font.bold: true }
                    RowLayout {
                Button { text: "Shuffle: " + (root.snapshot.queue && root.snapshot.queue.shuffled ? "On" : "Off"); enabled: !!root.snapshot.queue; onClicked: root.editQueue("set_shuffle", {enabled: !root.snapshot.queue.shuffled}) }
                Button { text: "Repeat: " + (root.snapshot.queue ? root.snapshot.queue.repeat : "off"); enabled: !!root.snapshot.queue; onClicked: { let modes = ["off", "one", "all"]; root.editQueue("set_repeat", {mode: modes[(modes.indexOf(root.snapshot.queue.repeat) + 1) % 3]}); } }
                    }
                    Label { text: root.snapshot.queue && root.snapshot.queue.navigating ? "Previous navigation · Next returns forward" : root.snapshot.queue && root.snapshot.queue.ended ? "End of selection · Play pending or Replay" : "Selected and upcoming"; color: "#acb8c8"; textFormat: Text.PlainText }
                    ListView {
                        id: queueView
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        model: root.queueModel
                        clip: true
                        spacing: 6
                        ScrollBar.vertical: ScrollBar {}
                        delegate: Rectangle {
                            required property var modelData
                            required property int index
                            width: queueView.width
                            height: 108
                            radius: 5
                            color: modelData.selected ? "#344354" : "#202735"
                            ColumnLayout {
                                anchors.fill: parent
                                anchors.margins: 8
                                spacing: 3
                                Label { text: (modelData.selected ? "Selected · " : "") + modelData.track.title; color: "#f2e9db"; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                                Label { text: modelData.track.artist; color: "#acb8c8"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                                RowLayout {
                                    Button { text: "Play"; enabled: !!root.snapshot.sign_in && root.snapshot.sign_in.connected; onClicked: root.editQueue("queue_play", {occurrence: modelData.id}) }
                                    Button { text: "↑"; enabled: modelData.can_move && index > 0 && root.queueModel[index - 1].can_move; onClicked: root.moveEntry(index, -1) }
                                    Button { text: "↓"; enabled: modelData.can_move && index + 1 < root.queueModel.length && root.queueModel[index + 1].can_move; onClicked: root.moveEntry(index, 1) }
                                    Button { text: "Remove"; onClicked: root.editQueue("queue_remove", {occurrence: modelData.id}) }
                                }
                            }
                        }
                    }
                    RowLayout {
                        Button { text: "Clear upcoming"; onClicked: root.editQueue("clear_upcoming") }
                        Button { text: "Stop & clear"; onClicked: root.editQueue("stop_clear") }
                    }
                    Label { visible: !!root.snapshot.queue && root.snapshot.queue.navigation_dropped !== "0"; text: "Previous retains the latest 4096 nominal visits; earlier visits were omitted."; color: "#acb8c8"; wrapMode: Text.Wrap; Layout.fillWidth: true }
                    Label { text: "Queue checkpoints restore paused after backend restart."; color: "#acb8c8"; wrapMode: Text.Wrap; Layout.fillWidth: true }
                }
            }
            Rectangle { Layout.fillWidth: true; height: 1; color: "#394355" }
            Label { text: root.snapshot.selection ? root.snapshot.selection.title + " · " + root.snapshot.selection.artist : "No track selected"; color: "#f2e9db"; textFormat: Text.PlainText; Layout.fillWidth: true; elide: Text.ElideRight }
            Flow {
                Layout.fillWidth: true
                Layout.preferredHeight: childrenRect.height
                spacing: 6
                Button { text: root.snapshot.intent === "playing" ? "Pause" : "Resume"; enabled: root.queueModel.length > 0 && !!root.snapshot.sign_in && root.snapshot.sign_in.connected; onClicked: root.send(root.snapshot.intent === "playing" ? "pause" : "resume") }
                Button { text: "Previous"; enabled: !!root.snapshot.queue && root.snapshot.queue.previous_available; onClicked: root.editQueue("previous") }
                Button { text: "Next track"; enabled: root.queueModel.length > 0; onClicked: root.editQueue("next") }
                Button { text: "Replay"; enabled: !!root.snapshot.queue && root.snapshot.queue.membership_count > 0; onClicked: root.editQueue("replay") }
                Label { text: "Seek (seconds)"; color: "#acb8c8" }
                TextField { id: seek; placeholderText: "0"; width: 75; validator: DoubleValidator { bottom: 0; top: 1000000000; locale: "C" } onAccepted: root.send("seek", {seconds: Number(text)}) }
                Button { text: "Seek"; enabled: !!root.snapshot.queue && !!root.snapshot.queue.selected && seek.acceptableInput; onClicked: root.send("seek", {seconds: Number(seek.text)}) }
                Label { width: parent.width; text: (root.snapshot.audio && root.snapshot.audio.position_seconds !== null ? root.snapshot.audio.position_seconds.toFixed(1) + " s · " : root.snapshot.queue && root.snapshot.queue.selected ? root.snapshot.queue.position_seconds.toFixed(1) + " s local · " : "") + "Desktop / PipeWire · ReplayGain off"; color: "#acb8c8" }
            }
        }
    }
    IpcHandler {
        target: "control"
        function show(): void { window.visible = true;  }
        function hide(): void { window.visible = false; }
        function browseAlbums(): void { root.queueRequested = false; albumBrowser.showDetails = !!albumBrowser.selected; libraryTabs.currentIndex = 2; window.visible = true; }
        function albumOverview(): void { root.queueRequested = false; albumBrowser.showDetails = false; libraryTabs.currentIndex = 2; window.visible = true; }
        function showQueue(): void { root.queueRequested = true; window.visible = true; }
        function browseArtists(): void { root.queueRequested = false; libraryTabs.currentIndex = 3; window.visible = true; }
        function browsePlaylists(): void { root.queueRequested = false; libraryTabs.currentIndex = 1; window.visible = true; }
        function status(): string { return JSON.stringify({artists: artistBrowser.artistModel.length, queueOnly: root.queueOnly, albums: {thumbnailsReady: albumBrowser.readyThumbnails, artistContext: !!albumBrowser.page && !!albumBrowser.page.artist,details: albumBrowser.showDetails, count: albumBrowser.albumModel.length, tracks: albumBrowser.trackModel.length, coverReady: albumBrowser.coverReady, trackViewport: albumBrowser.trackViewport}, geometry: {width: window.width, height: window.height, playlistHeight: playlistTracks.height}, connected: socket.connected, instance: root.instance, playlists: root.snapshot.playlists ? {busy: root.snapshot.playlists.busy, uncertain: root.snapshot.playlists.uncertain, lists: root.playlistModel.length, entries: root.playlistEntries.length} : null, storage: root.snapshot.storage || null, queue: root.snapshot.queue ? {shuffled: root.snapshot.queue.shuffled, repeat: root.snapshot.queue.repeat, previousAvailable: root.snapshot.queue.previous_available, position: root.snapshot.queue.position_seconds, session: root.snapshot.queue.session, revision: root.snapshot.queue.revision, entries: root.snapshot.queue.entries.map(row => ({id: row.id, selected: row.selected, key: row.track.reference.key})), ended: root.snapshot.queue.ended} : null, signIn: root.snapshot.sign_in || null, libraries: (root.snapshot.libraries || []).length, tracks: root.snapshot.page ? root.snapshot.page.tracks.length : 0, visible: window.visible}); }
        function quit(): void { root.send("quit"); Qt.quit(); }
    }
}
