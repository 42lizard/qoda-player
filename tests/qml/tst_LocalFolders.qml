import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "LocalFolders"
    width: 500; height: 700; visible: true; when: windowShown
    Qoda.LocalFolders { id: folders; anchors.fill: parent; connected: true; snapshot: null }
    SignalSpy { id: applied; target: folders; signalName: "applyRequested" }
    SignalSpy { id: rescanned; target: folders; signalName: "rescanRequested" }
    function frame() { return {available:true, defaulted:true, phase:"ready", paths:["/Music Ä"], folders:[{id:"local", path:"/Music Ä", tracks:"1", message:"Local folder ready."}], message:"Local folder ready."}; }
    function init() { folders.connected=true;folders.snapshot=null;folders.snapshot=frame();applied.clear();rescanned.clear();waitForPolish(folders); }
    function press(name) { const item=findChild(folders,name);verify(!!item);item.forceActiveFocus();keyClick(Qt.Key_Space); }
    function test_add_remove_and_apply_are_explicit_without_changing_music() {
        const input=findChild(folders,"newLocalFolder");input.text="/Another folder";press("addLocalFolder");compare(applied.count,0);compare(folders.paths().length,2);
        press("applyLocalFolders");compare(applied.count,1);compare(applied.signalArguments[0][0][0],"/Music Ä");compare(applied.signalArguments[0][0][1],"/Another folder");
        press("removeLocalFolder");compare(applied.count,1);compare(folders.paths().length,1);
    }
    function test_watch_status_preserves_draft_and_disconnect_blocks_mutation() {
        const input=findChild(folders,"newLocalFolder");input.text="/draft";press("addLocalFolder");let next=frame();next.phase="scanning";folders.snapshot=next;compare(folders.paths().length,2);verify(findChild(folders,"applyLocalFolders").enabled);verify(!findChild(folders,"rescanLocalFolders").enabled);
        folders.connected=false;verify(!findChild(folders,"applyLocalFolders").enabled);verify(!findChild(folders,"addLocalFolder").enabled);
        folders.connected=true;folders.snapshot=frame();press("rescanLocalFolders");compare(rescanned.count,1);compare(applied.count,0);
    }
    function test_empty_configuration_can_be_saved_and_localized() {
        Qoda.I18n.preferences=["de-DE"];
        press("removeLocalFolder");compare(folders.paths().length,0);compare(findChild(folders,"applyLocalFolders").text,"Ordner übernehmen");press("applyLocalFolders");compare(applied.count,1);compare(applied.signalArguments[0][0].length,0);
        Qoda.I18n.preferences=Qt.locale().uiLanguages;
    }
}
