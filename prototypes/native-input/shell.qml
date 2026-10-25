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
    property var inputEvents: []
    property var reviews: []
    property int step: 0
    property var cases: [
        "Tab and Shift+Tab", "Spaces inside search", "Ctrl+K from full",
        "Enter activates a focused button", "Space on bare Key test surface",
        "Escape: dialog, drawer, full", "Compositor close and restore",
        "Compact outside click and keyboard release", "Ctrl+K from compact",
        "Compact dialog Escape, hide, focus return and restore",
        "Narrow layout visual review", "Wide layout visual review"]
    property var instructions: [
        "Click Search. Press Tab: focus should move to Queue. Press Shift+Tab: Search should regain focus.",
        "In Search, type only cedar river (including the space). The fake Play/Pause state must not change.",
        "Click Key test, then press Ctrl+K. Search should gain focus.",
        "Focus Key test with Tab/Shift+Tab, then press Enter. The bare surface should gain focus. Watch the focus readout.",
        "Click Key test, then press Space once. Fake Play/Pause should toggle once. No audio is played.",
        "Open Test dialog and press Escape: only the dialog closes. Open Queue and press Escape: only the drawer closes. Click Key test and press Escape: full stays open.",
        "With player focused, press Super+W (compositor Close window). Player should hide. Click Restore below: full returns, preserving Artists, selection, search and scroll.",
        "Click Open compact below. Click Outside target below: panel stays visible, helper gains focus. Press Tab in this helper: its focus must move, with no panel keyboard grab.",
        "Click compact Play/Pause, then Ctrl+K: full Search should gain focus. Use only fictional data.",
        "Click Open compact. Open its Test dialog, Escape closes only dialog. Escape again hides compact and returns focus to this helper. Click Restore: compact returns.",
        "Click Prepare case below to show normal full. Review narrow layout: usable search, list and controls; Queue opens a drawer. If full width is 1050 or more this case is unavailable, not a pass.",
        "Click Prepare case for temporary fullscreen. Review persistent queue and all controls at 200% scale. Use Finish wide review inside the player to return to the helper. No desktop config changes."]
    function observe(event, source) {
        if (event.isAutoRepeat) return;
        let name = ({[Qt.Key_Tab]:"Tab",[Qt.Key_Backtab]:"Shift+Tab",[Qt.Key_Return]:"Enter",[Qt.Key_Enter]:"Enter",[Qt.Key_Space]:"Space",[Qt.Key_Escape]:"Escape"})[event.key];
        if (event.key === Qt.Key_Tab && (event.modifiers & Qt.ShiftModifier)) name = "Shift+Tab";
        if (name) input("key",source,name);
    }
    function input(kind,source,key) {
        inputEvents=inputEvents.concat([{time:Date.now(),step:step,kind:kind,source:source,key:key}]).slice(-500);
    }
    component ObservedButton: C.Button {
        id: button
        Keys.priority: Keys.BeforeItem
        Keys.onPressed: event => {
            root.observe(event, objectName || "button");
            if (!event.isAutoRepeat && (event.key===Qt.Key_Return || event.key===Qt.Key_Enter)) {
                button.clicked(); event.accepted=true;
            }
        }
        TapHandler { onTapped:root.input("pointer",button.objectName || "button","") }
    }
    function prepare() {
        if(step===0) {
            root.page="Artists";root.selected=25;root.query="";
            root.showForm("full",true);
            Qt.callLater(()=>{libraryList.positionViewAtIndex(25,ListView.Beginning);root.savedY=libraryList.contentY;});
        } else if(step===10 || step===11) {
            root.showForm("full",true);full.fullscreen=step===11;
        }
        root.record("fixture:prepare:"+step);
    }
    function review(result) {
        reviews=reviews.concat([{step:step,name:cases[step],humanReport:result,time:Date.now(),state:JSON.parse(snapshot())}]);
        if(step<cases.length-1) step++;
        else root.record("checklist-reviewed");
    }


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
            if(event.isAutoRepeat) {event.accepted=true;return;}
            physicalHandledKeys++; handleEscape("key"); event.accepted = true;
        } else if (event.key === Qt.Key_Space) {
            if(event.isAutoRepeat) {event.accepted=true;return;}
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
        return JSON.stringify({form:form,lastForm:lastForm,page:page,queryMatchesFixture:query === "cedar river",queryEmpty:query.length===0,selected:selected,
            savedY:savedY,listY:libraryList.contentY,playing:playing,ticks:ticks,closeEvents:closeEvents,
            handledPhysicalKeys:physicalHandledKeys,dialog:dialog.visible,queueDrawer:queueDrawer.visible,
            full:{visible:full.visible,mapped:full.backingWindowVisible,width:full.width,height:full.height,
                dpr:full.devicePixelRatio,active:full.contentItem.Window.window ? full.contentItem.Window.window.active : false,
                focusItem:activeItem(full),searchFocused:fullSearch.activeFocus,
                persistentQueue:persistentQueue.visible,screen:full.screen ? full.screen.name : null},
            compact:{visible:compact.visible,mapped:compact.backingWindowVisible,width:compact.width,height:compact.height,
                dpr:compact.devicePixelRatio,active:compact.contentItem.Window.window ? compact.contentItem.Window.window.active : false,
                focusItem:activeItem(compact),keyboardMode:compact.WlrLayershell.keyboardFocus,
                screen:compact.screen ? compact.screen.name : null},helperActive:helper.contentItem.Window.window ? helper.contentItem.Window.window.active : false,step:step,inputEvents:inputEvents,reviews:reviews.map(r=>({step:r.step,name:r.name,humanReport:r.humanReport,time:r.time})),screens:Quickshell.screens.map(s=>({name:s.name,width:s.width,height:s.height})),journal:journal});
    }

    Timer { interval:1000; repeat:true; running:true; onTriggered: if(root.playing) root.ticks++; }
    Timer { id:prime; interval:90; onTriggered: { if(compact.visible && root.wantFocus) {root.primed=true; compactPlay.forceActiveFocus(); root.record("focus:ondemand");} } }

    IpcHandler {
        target: "inputStudy"
        function snapshot(): string { return root.snapshot(); }
        function reviews(): string { return JSON.stringify(root.reviews); }
        function quit(): void { Qt.quit(); }
    }

    FloatingWindow {
        id:full
        title:"Qoda input check · fictional library"
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
            Keys.onPressed:event=>{root.observe(event,root.form+"-surface");root.handleKey(event);}
            Shortcut { sequence:"Ctrl+K"; enabled:full.visible; onActivated:{root.input("shortcut","full","Ctrl+K");root.search();} }

            ColumnLayout {
                anchors.fill:parent; anchors.margins:16; spacing:12
                RowLayout {
                    Layout.fillWidth:true
                    Text {text:"QODA / input check"; color:"#c7e9a3"; font.bold:true; Layout.fillWidth:true; elide:Text.ElideRight}
                    ObservedButton {objectName:"full-compact"; text:"Compact"; onClicked:root.showForm("compact",true)}
                    ObservedButton {objectName:"full-hide"; text:"Hide"; onClicked:root.hide()}
                    ObservedButton {objectName:"key-probe"; text:"Key test"; onClicked:{root.input("activate","key-probe","");fullScope.forceActiveFocus()}}
                    ObservedButton {objectName:"full-quit"; text:"Quit"; onClicked:{root.playing=false; Qt.quit()}}
                }
                RowLayout {
                    Layout.fillWidth:true
                    C.ComboBox {objectName:"library"; model:["Home / Music","Juniper / Shared music"]; Layout.preferredWidth:160; onActivated:root.library=currentText}
                    C.TextField {id:fullSearch; objectName:"search"; Layout.fillWidth:true; placeholderText:"Search · Ctrl+K"; text:root.query; onTextEdited:root.query=text; Keys.priority:Keys.BeforeItem; Keys.onPressed:event=>root.observe(event,"search")}
                    ObservedButton {objectName:"queue"; text:"Queue"; onClicked:queueDrawer.open()}
                }
                Text {text:"No audio · full/compact/hidden preserve the fake listening state"; color:"#a9b8a4"; font.pixelSize:12; Layout.fillWidth:true; wrapMode:Text.Wrap}
                RowLayout {
                    Layout.fillWidth:true; Layout.fillHeight:true; spacing:16
                    ColumnLayout {
                        Layout.preferredWidth:100; Layout.alignment:Qt.AlignTop
                        Repeater {
                            model:["Albums","Artists","Playlists"]
                            ObservedButton {required property string modelData; text:modelData; highlighted:root.page===modelData; Layout.fillWidth:true; onClicked:root.page=modelData}
                        }
                        ObservedButton {objectName:"dialog"; text:"Test dialog"; Layout.fillWidth:true; onClicked:root.openDialog()}
                    }
                    ColumnLayout {
                        Layout.fillWidth:true; Layout.fillHeight:true
                        Text {text:root.page; color:"#edf3e9"; font.pixelSize:23; font.bold:true}
                        ListView {
                            id:libraryList
                            Layout.fillWidth:true; Layout.fillHeight:true; clip:true; spacing:4
                            model:80
                            onContentYChanged: if(full.visible) root.savedY=contentY
                            delegate:ObservedButton {
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
                ObservedButton {
                    visible:root.step===11; text:"Finish wide review · return to checklist"; Layout.fillWidth:true
                    onClicked:{full.fullscreen=false;root.showForm("hidden",false);helper.contentItem.Window.window.requestActivate();}
                }
                RowLayout {
                    Layout.fillWidth:true
                    ObservedButton {objectName:"full-play"; text:root.playing?"Pause":"Play"; onClicked:root.playing=!root.playing}
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
            Keys.onPressed:event=>{root.observe(event,root.form+"-surface");root.handleKey(event);}
            Shortcut {sequence:"Ctrl+K"; enabled:compact.visible; onActivated:{root.input("shortcut","compact","Ctrl+K");root.search();}}
            TapHandler {onPressedChanged:if(pressed && !root.wantFocus){root.wantFocus=true;root.primed=false;prime.restart()}}
            ColumnLayout {
                anchors.fill:parent; anchors.margins:18; spacing:12
                RowLayout {
                    Layout.fillWidth:true
                    Text {text:"Qoda · compact study"; color:"#c7e9a3"; font.bold:true; Layout.fillWidth:true}
                    ObservedButton {objectName:"compact-expand"; text:"Expand"; onClicked:root.showForm("full",true)}
                    ObservedButton {objectName:"compact-hide"; text:"×"; onClicked:root.hide()}
                }
                Text {text:"Open water"; color:"#edf3e9"; font.pixelSize:24; font.bold:true}
                Text {text:"Mira Vale · fake playback "+root.ticks+" s"; color:"#afc3a6"}
                RowLayout {
                    ObservedButton {id:compactPlay; objectName:"compact-play"; text:root.playing?"Pause":"Play"; onClicked:root.playing=!root.playing}
                    ObservedButton {objectName:"compact-dialog"; text:"Test dialog"; onClicked:root.openDialog()}
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
        contentItem:C.Label {Keys.priority:Keys.BeforeItem; Keys.onPressed:event=>root.observe(event,"dialog-content"); text:"Escape dismisses this dialog before hiding the compact panel. No real operation occurs."; wrapMode:Text.Wrap}
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
            Keys.priority:Keys.BeforeItem; Keys.onPressed:event=>root.observe(event,"queue-content");
            anchors.fill:parent; anchors.margins:20; spacing:16
            C.Label {text:"Listening queue"; font.pixelSize:22}
            Repeater {model:["Open water · current","Half-light","A place to return","Between the lines"]; C.Label {required property string modelData; text:modelData}}
            ObservedButton {text:"Close queue"; onClicked:queueDrawer.close()}
        }
        onClosed:root.record("queue-closed")
    }

    FloatingWindow {
        id:helper
        title:"Qoda · physical input checklist"
        visible:true; implicitWidth:430; implicitHeight:640
        minimumSize:Qt.size(320,440); color:"#172229"
        onClosed:Qt.quit()
        FocusScope {
            anchors.fill:parent; focus:true
            Keys.priority:Keys.AfterItem
            Keys.onPressed:event=>root.observe(event,"checklist-surface")
            ColumnLayout {
                anchors.fill:parent; anchors.margins:16; spacing:12
                C.Label {text:"Physical input check · no audio"; color:"#bce7d0"; font.bold:true; Layout.fillWidth:true; wrapMode:Text.Wrap}
                C.Label {text:(root.step+1)+" / "+root.cases.length+" · "+root.cases[root.step]; color:"white"; font.pixelSize:18; Layout.fillWidth:true; wrapMode:Text.Wrap}
                C.Label {text:root.instructions[root.step]; color:"#deeadf"; Layout.fillWidth:true; wrapMode:Text.Wrap}
                RowLayout {
                    ObservedButton {objectName:"prepare-case";text:root.step===0?"Start / Prepare":"Prepare case";onClicked:root.prepare()}
                    ObservedButton {objectName:"outside-target";text:"Outside target";onClicked:root.input("activate","outside-target","")}
                }
                RowLayout {
                    ObservedButton {objectName:"restore";text:"Restore";onClicked:root.toggle()}
                    ObservedButton {objectName:"open-compact";text:"Open compact";onClicked:root.showForm("compact",true)}
                }
                C.Label {text:"Confirm only after performing the action. Reports are human observations, not automated passes. Use Issue / unavailable if it fails or cannot be performed."; color:"#e7cfa8"; Layout.fillWidth:true;wrapMode:Text.Wrap}
                RowLayout {
                    ObservedButton {objectName:"report-ok";text:"Observed OK →";onClicked:root.review("observed-ok")}
                    ObservedButton {objectName:"report-issue";text:"Issue / unavailable →";onClicked:root.review("issue-or-unavailable")}
                }
                C.Label {
                    Layout.fillWidth:true; Layout.fillHeight:true; wrapMode:Text.Wrap; color:"#bdd0dd"
                    text:"Player: "+root.form+" · "+root.activeItem(full)+"\nCompact focus: "+root.activeItem(compact)+"\nSearch matches fixture: "+(root.query==="cedar river")+"\nNative close events: "+root.closeEvents+"\nRecorded input events: "+root.inputEvents.length+"\nHuman reviews: "+root.reviews.length+"\n\nOnly eDP-1 at 200% is available. Other displays, scale factors and hotplug remain unverified."
                }
                ObservedButton {text:"Quit this check";onClicked:Qt.quit()}
            }
        }
    }
    Component.onCompleted:root.record("ready:hidden")
}
