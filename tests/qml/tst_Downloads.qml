import QtQuick
import QtTest
import "../../ui" as Qoda
TestCase {
    name: "Downloads"
    width: 500; height: 850; visible: true; when: windowShown
    Qoda.Downloads { id: downloads; anchors.fill: parent; connected: true; snapshot: null; folders: [{id:"root",path:"/Music"}] }
    SignalSpy { id: actions; target: downloads; signalName: "actionRequested" }
    function frame(phase,local) { return {available:true,destination_revision:"9007199254740993",destination:{source:"root",path:"/Music/Qoda Downloads"},message:"",jobs:[{id:"job",track:{title:"Original",artist:"Fixture",album:"Fixture"},phase:phase,bytes:"1024",total:"2048",message:"",local:local ? {source:"root",key:"local"} : null,missing:false}]}; }
    function init() { downloads.connected=true;downloads.snapshot=frame("paused",false);actions.clear();waitForPolish(downloads); }
    function press(name) { const item=findChild(downloads,name);verify(!!item);item.forceActiveFocus();keyClick(Qt.Key_Space); }
    function test_destination_continue_and_cancel_are_explicit_job_actions() {
        press("useDownloadFolder");compare(actions.count,1);compare(actions.signalArguments[0][0],"download_destination");compare(actions.signalArguments[0][1].source,"root");
        press("continueDownload");compare(actions.signalArguments[1][0],"download_retry");compare(actions.signalArguments[1][1].job_id,"job");
        press("cancelDownload");compare(actions.signalArguments[2][0],"download_cancel");
    }
    function test_completed_local_copy_is_explicit_and_missing_or_disconnected_is_disabled() {
        downloads.snapshot=frame("completed",true);waitForPolish(downloads);press("queueLocalDownload");compare(actions.signalArguments[0][0],"download_local");compare(actions.signalArguments[0][1].job_id,"job");
        downloads.snapshot=frame("completed",false);waitForPolish(downloads);verify(!findChild(downloads,"queueLocalDownload").enabled);
        downloads.connected=false;verify(!findChild(downloads,"clearDownloads").enabled);verify(!findChild(downloads,"useDownloadFolder").enabled);
    }
    function test_unsupported_storage_and_german_controls() {
        downloads.snapshot={available:false,jobs:[],message:"Unsupported download history was preserved; use a compatible Qoda version."};waitForPolish(downloads);verify(!findChild(downloads,"useDownloadFolder").enabled);
        Qoda.I18n.preferences=["de-DE"];compare(findChild(downloads,"clearDownloads").text,"Erledigte Aufträge ausblenden");Qoda.I18n.preferences=Qt.locale().uiLanguages;
    }
}
