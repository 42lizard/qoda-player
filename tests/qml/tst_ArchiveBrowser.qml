import QtQuick
import QtQuick.Controls
import QtTest
import "../../ui" as Qoda
TestCase {
    id: test
    name: "ListeningArchive"
    width: 440; height: 740
    visible: true; when: windowShown
    Qoda.ArchiveBrowser { id: browser; anchors.fill: parent; snapshot: null; connected: true; accountEpoch: "9007199254740993" }
    SignalSpy { id: requests; target: browser; signalName: "requested" }
    function frame() { return {available:true,cleanup_pending:false,retention_days:90,total:42,offset:20,has_next:true,message:"Synthetic nominal history",entries:[{visit:"visit-a",title:"Fictional duplicate",artist:"Fixture",album:"Original synthetic",track:{source:"fixture"},first_observed_ms:1700000000000,last_position_ms:1200,max_position_ms:9000}],export_path:null}; }
    function init() { browser.connected=true;browser.accountEpoch="9007199254740993";browser.snapshot=frame();requests.clear();waitForPolish(browser); }
    function activate(name) { const control=findChild(browser,name);verify(!!control);control.forceActiveFocus();keyClick(Qt.Key_Space); }
    function test_pages_and_export_keep_account_fence() {
        activate("olderArchive");compare(requests.count,1);compare(requests.signalArguments[0][0],"archive_page");compare(requests.signalArguments[0][1].offset,40);compare(requests.signalArguments[0][1].account_epoch,"9007199254740993");
        activate("exportArchive");compare(requests.count,2);compare(requests.signalArguments[1][0],"archive_export");
    }
    function test_clear_needs_dialog_and_account_change_closes_it() {
        activate("clearArchive");compare(requests.count,0);const dialog=findChild(browser,"confirmArchiveClear");tryCompare(dialog,"visible",true);browser.accountEpoch="new-account";tryCompare(dialog,"visible",false);compare(requests.count,0);
        activate("clearArchive");tryCompare(dialog,"visible",true);dialog.accept();compare(requests.count,1);compare(requests.signalArguments[0][0],"archive_clear");
    }
    function test_clock_updates_do_not_reset_a_retention_draft() {
        browser.draftDays=7;let updated=frame();updated.total=43;browser.snapshot=updated;compare(browser.draftDays,7);
        activate("applyArchiveRetention");compare(requests.count,1);compare(requests.signalArguments[0][0],"archive_retention");compare(requests.signalArguments[0][1].days,7);
    }
    function test_disabled_and_cleanup_states_gate_actions() {
        browser.connected=false;verify(!findChild(browser,"exportArchive").enabled);verify(!findChild(browser,"olderArchive").enabled);browser.request("archive_export",{});compare(requests.count,0);
        browser.connected=true;let blocked=frame();blocked.available=false;blocked.cleanup_pending=true;browser.snapshot=blocked;
        verify(!findChild(browser,"exportArchive").enabled);activate("clearArchive");compare(requests.count,1);compare(requests.signalArguments[0][0],"archive_clear");
    }
}
