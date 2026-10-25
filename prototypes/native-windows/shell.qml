// THROWAWAY native surface study. No Plex, audio, persistence, desktop bindings or config edits.
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Controls as C
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Hyprland

ShellRoot {
    id: root
    property string form: "hidden"
    property string lastForm: "full"
    property string page: "Albums"
    property string library: "Home / Music"
    property string query: ""
    property int selected: 0
    property real savedY: 0
    property bool playing: true
    property int ticks: 0
    property bool wantFocus: false
    property bool primed: false
    property int closeEvents: 0
    property int physicalHandledKeys: 0
    property var targetScreen: Quickshell.screens[0] ?? null
    property var journal: []

    function record(action) {
        journal = journal.concat([{action: action, form: form, tick: ticks}]).slice(-60);
        console.log("QODA_NATIVE", action, form);
    }
    function screenForInvocation() {
        const name = Hyprland.focusedMonitor ? Hyprland.focusedMonitor.name : "";
        return Quickshell.screens.find(s => s.name === name) ?? Quickshell.screens[0] ?? null;
    }
    function activateFull() {
        if (form !== "full" || !full.visible) return;
        if (full.contentItem.Window.window) full.contentItem.Window.window.requestActivate();
        fullSearch.forceActiveFocus();
        libraryList.contentY = savedY;
    }
    function showForm(next, focusRequested) {
        if (full.visible) savedY = libraryList.contentY;
        if (form !== "hidden") lastForm = form;
        dialog.close(); queueDrawer.close();
        wantFocus = false;
        prime.stop();
        const invocationScreen = screenForInvocation();
        form = next;
        full.visible = next === "full";
        compact.visible = false;
        if (next === "compact") {
            targetScreen = invocationScreen;
            primed = false;
            wantFocus = focusRequested;
            compact.visible = true;
            if (focusRequested) prime.restart();
        }
        if (next !== "hidden") lastForm = next;
        if (next === "full") Qt.callLater(activateFull);
        record("show:" + next + (focusRequested ? ":interactive" : ":passive"));
    }
    function hide() { showForm("hidden", false); }
    function toggle() { if (form === "hidden") showForm(lastForm, true); else hide(); }
    function handleEscape(origin) {
        if (dialog.visible) { dialog.close(); record("escape:dialog:" + origin); }
        else if (queueDrawer.visible) { queueDrawer.close(); record("escape:queue:" + origin); }
        else if (form === "compact") { hide(); record("escape:compact:" + origin); }
        else record("escape:full-stays-open:" + origin);
    }
    function handleKey(event) {
        if (event.key === Qt.Key_Escape) {
            physicalHandledKeys++; escape("key"); event.accepted = true;
        } else if (event.key === Qt.Key_Space) {
            physicalHandledKeys++; playing = !playing; record("space:playback"); event.accepted = true;
        }
    }
    function search() {
        if (form !== "full") showForm("full", true);
        Qt.callLater(() => fullSearch.forceActiveFocus());
        record("search-focus");
    }
    function openDialog() { dialog.open(); record("dialog-open"); }
    function activeItem(window) {
        const item = window.contentItem.Window.window ? window.contentItem.Window.window.activeFocusItem : null;
        return item ? (item.objectName || "Qt internal focus item") : "none";
    }
    function snapshot() {
        return JSON.stringify({form:form,lastForm:lastForm,page:page,library:library,query:query,selected:selected,
            savedY:savedY,listY:libraryList.contentY,playing:playing,ticks:ticks,closeEvents:closeEvents,
            handledPhysicalKeys:physicalHandledKeys,dialog:dialog.visible,queueDrawer:queueDrawer.visible,
            full:{visible:full.visible,mapped:full.backingWindowVisible,width:full.width,height:full.height,
                dpr:full.devicePixelRatio,active:full.contentItem.Window.window ? full.contentItem.Window.window.active : false,
                focusItem:activeItem(full),searchFocused:fullSearch.activeFocus,
                persistentQueue:persistentQueue.visible,screen:full.screen ? full.screen.name : null},
            compact:{visible:compact.visible,mapped:compact.backingWindowVisible,width:compact.width,height:compact.height,
                dpr:compact.devicePixelRatio,active:compact.contentItem.Window.window ? compact.contentItem.Window.window.active : false,
                focusItem:activeItem(compact),keyboardMode:compact.WlrLayershell.keyboardFocus,
                screen:compact.screen ? compact.screen.name : null},screens:Quickshell.screens.map(s=>({name:s.name,width:s.width,height:s.height})),journal:journal});
    }

    Timer { interval:1000; repeat:true; running:true; onTriggered: if(root.playing) root.ticks++; }
    Timer { id:prime; interval:90; onTriggered: { if(compact.visible && root.wantFocus) {root.primed=true; compactPlay.forceActiveFocus(); root.record("focus:ondemand");} } }

    IpcHandler {
        target: "probe"
        function snapshot(): string { return root.snapshot(); }
        function showFull(): void { root.showForm("full",true); }
        function showCompact(interactive: bool): void { root.showForm("compact",interactive); }
        function hide(): void { root.hide(); }
        function toggle(): void { root.toggle(); }
        function openDialog(): void { root.openDialog(); }
        function dismiss(): void { root.handleEscape("ipc"); }
        function openQueue(): void { queueDrawer.open(); root.record("queue-open"); }
        function seed(page: string, query: string, index: int): void {
            root.page=page; root.query=query; root.selected=index;
            libraryList.positionViewAtIndex(index,ListView.Beginning);
            root.savedY=libraryList.contentY; root.record("seed-navigation");
        }
        function maximize(value: bool): void { full.maximized=value; }
        function wideProbe(value: bool): void { full.fullscreen=value; root.record("wide-probe:"+value); }
        function closeFull(): void { if(full.contentItem.Window.window) full.contentItem.Window.window.close(); }
        function focusNext(): string {
            const win=root.form === "compact" ? compact : full;
            const item=win.contentItem.Window.window ? win.contentItem.Window.window.activeFocusItem : null;
            if(item) item.nextItemInFocusChain(true).forceActiveFocus(Qt.TabFocusReason);
            return root.activeItem(win);
        }
        function quit(): void { root.playing=false; root.record("quit"); Qt.quit(); }
    }

    FloatingWindow {
        id:full
        title:"Qoda Native Prototype · fictional library"
        visible:false
        implicitWidth:1100; implicitHeight:700
        minimumSize:Qt.size(480,320)
        color:"#151d17"
        onClosed: {root.closeEvents++; root.hide(); root.record("native-close-as-hide");}
        onWindowConnected: Qt.callLater(root.activateFull)

        FocusScope {
            id:fullScope
            anchors.fill:parent
            focus:true
            Keys.priority:Keys.AfterItem
            Keys.onPressed:event=>root.handleKey(event)
            Shortcut { sequence:"Ctrl+K"; enabled:full.visible; onActivated:root.search(); }

            ColumnLayout {
                anchors.fill:parent; anchors.margins:16; spacing:12
                RowLayout {
                    Layout.fillWidth:true
                    Text {text:"QODA / native study"; color:"#c7e9a3"; font.bold:true; Layout.fillWidth:true; elide:Text.ElideRight}
                    C.Button {objectName:"full-compact"; text:"Compact"; onClicked:root.showForm("compact",true)}
                    C.Button {objectName:"full-hide"; text:"Hide"; onClicked:root.hide()}
                    C.Button {objectName:"key-probe"; text:"Key test"; onClicked:fullScope.forceActiveFocus()}
                    C.Button {objectName:"full-quit"; text:"Quit"; onClicked:{root.playing=false; Qt.quit()}}
                }
                RowLayout {
                    Layout.fillWidth:true
                    C.ComboBox {objectName:"library"; model:["Home / Music","Juniper / Shared music"]; Layout.preferredWidth:160; onActivated:root.library=currentText}
                    C.TextField {id:fullSearch; objectName:"search"; Layout.fillWidth:true; placeholderText:"Search · Ctrl+K"; text:root.query; onTextEdited:root.query=text}
                    C.Button {objectName:"queue"; text:"Queue"; onClicked:queueDrawer.open()}
                }
                Text {text:"No audio · full/compact/hidden preserve the fake listening state"; color:"#a9b8a4"; font.pixelSize:12; Layout.fillWidth:true; wrapMode:Text.Wrap}
                RowLayout {
                    Layout.fillWidth:true; Layout.fillHeight:true; spacing:16
                    ColumnLayout {
                        Layout.preferredWidth:100; Layout.alignment:Qt.AlignTop
                        Repeater {
                            model:["Albums","Artists","Playlists"]
                            C.Button {required property string modelData; text:modelData; highlighted:root.page===modelData; Layout.fillWidth:true; onClicked:root.page=modelData}
                        }
                        C.Button {objectName:"dialog"; text:"Test dialog"; Layout.fillWidth:true; onClicked:root.openDialog()}
                    }
                    ColumnLayout {
                        Layout.fillWidth:true; Layout.fillHeight:true
                        Text {text:root.page; color:"#edf3e9"; font.pixelSize:23; font.bold:true}
                        ListView {
                            id:libraryList
                            Layout.fillWidth:true; Layout.fillHeight:true; clip:true; spacing:4
                            model:80
                            onContentYChanged: if(full.visible) root.savedY=contentY
                            delegate:C.Button {
                                required property int index
                                objectName:"item-"+index
                                width:ListView.view.width; height:44
                                text:(root.page === "Albums" ? "Soft geometry / Album " : root.page+" / Item ")+(index+1)
                                highlighted:root.selected===index
                                onClicked:{root.selected=index; root.record("select:"+index)}
                            }
                            C.ScrollBar.vertical:C.ScrollBar {}
                        }
                    }
                    Rectangle {
                        id:persistentQueue
                        visible:full.width>=1050
                        Layout.preferredWidth:230; Layout.fillHeight:true
                        color:"#202d22"; radius:8
                        Column {
                            anchors.fill:parent; anchors.margins:15; spacing:16
                            Text {text:"Queue"; color:"#d9edc5"; font.bold:true; font.pixelSize:19}
                            Repeater {model:["Open water · now","Half-light","A place to return","Between the lines"]; Text {required property string modelData; text:modelData; color:"#c4d2bf"; width:parent.width; wrapMode:Text.Wrap}}
                        }
                    }
                }
                RowLayout {
                    Layout.fillWidth:true
                    C.Button {objectName:"full-play"; text:root.playing?"Pause":"Play"; onClicked:root.playing=!root.playing}
                    Text {text:"Open water · Mira Vale"; color:"#e1ecd8"; Layout.fillWidth:true; elide:Text.ElideRight}
                    Text {text:"Fake time "+root.ticks+" s"; color:"#c7e9a3"}
                }
            }
        }
    }

    PanelWindow {
        id:compact
        visible:false
        screen:root.targetScreen
        implicitWidth:Math.min(420,(screen?screen.width:440)-24)
        implicitHeight:240
        color:"#1d2b20"
        anchors {bottom:true; right:true}
        margins {bottom:18; right:18}
        exclusionMode:ExclusionMode.Ignore
        WlrLayershell.namespace:"qoda-native-prototype"
        WlrLayershell.layer:WlrLayer.Overlay
        WlrLayershell.keyboardFocus:!visible || !root.wantFocus ? WlrKeyboardFocus.None : root.primed ? WlrKeyboardFocus.OnDemand : WlrKeyboardFocus.Exclusive
        onBackingWindowVisibleChanged: if(backingWindowVisible && root.wantFocus) prime.restart()
        FocusScope {
            anchors.fill:parent; focus:true
            Keys.priority:Keys.AfterItem
            Keys.onPressed:event=>root.handleKey(event)
            Shortcut {sequence:"Ctrl+K"; enabled:compact.visible; onActivated:root.search()}
            TapHandler {onPressedChanged:if(pressed && !root.wantFocus){root.wantFocus=true;root.primed=false;prime.restart()}}
            ColumnLayout {
                anchors.fill:parent; anchors.margins:18; spacing:12
                RowLayout {
                    Layout.fillWidth:true
                    Text {text:"Qoda · compact study"; color:"#c7e9a3"; font.bold:true; Layout.fillWidth:true}
                    C.Button {objectName:"compact-expand"; text:"Expand"; onClicked:root.showForm("full",true)}
                    C.Button {objectName:"compact-hide"; text:"×"; onClicked:root.hide()}
                }
                Text {text:"Open water"; color:"#edf3e9"; font.pixelSize:24; font.bold:true}
                Text {text:"Mira Vale · fake playback "+root.ticks+" s"; color:"#afc3a6"}
                RowLayout {
                    C.Button {id:compactPlay; objectName:"compact-play"; text:root.playing?"Pause":"Play"; onClicked:root.playing=!root.playing}
                    C.Button {objectName:"compact-dialog"; text:"Test dialog"; onClicked:root.openDialog()}
                    Text {text:root.wantFocus?"Esc hides":"Passive preview"; color:"#b2c5a9"}
                }
            }
        }
    }

    C.Dialog {
        id:dialog
        parent:root.form === "compact" ? compact.contentItem : full.contentItem
        width:Math.min(340,parent.width-24)
        x:(parent.width-width)/2; y:(parent.height-height)/2
        title:"Dialog has Escape priority"
        modal:true; focus:true
        standardButtons:C.Dialog.Ok | C.Dialog.Cancel
        closePolicy:C.Popup.CloseOnEscape
        contentItem:C.Label {text:"Escape dismisses this dialog before hiding the compact panel. No real operation occurs."; wrapMode:Text.Wrap}
        onClosed:root.record("dialog-closed")
    }
    C.Drawer {
        id:queueDrawer
        parent:full.contentItem
        edge:Qt.RightEdge
        width:Math.min(330,full.width-24); height:full.height
        modal:true; focus:true
        closePolicy:C.Popup.CloseOnEscape | C.Popup.CloseOnPressOutside
        Column {
            anchors.fill:parent; anchors.margins:20; spacing:16
            C.Label {text:"Listening queue"; font.pixelSize:22}
            Repeater {model:["Open water · current","Half-light","A place to return","Between the lines"]; C.Label {required property string modelData; text:modelData}}
            C.Button {text:"Close queue"; onClicked:queueDrawer.close()}
        }
        onClosed:root.record("queue-closed")
    }
    Component.onCompleted:root.record("ready:hidden")
}
