import QtQuick
import QtQuick.Controls
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "OfflineDiscovery"
    width: 420; height: 180; visible: true; when: windowShown
    Qoda.DiscoveryNotice { id: notice; width: 400 }
    Qoda.LibraryPicker { id: picker; visible: false; width: 180; libraries: []; selectedId: "" }
    SignalSpy { id: reconnects; target: notice; signalName: "reconnectRequested" }
    SignalSpy { id: signins; target: notice; signalName: "signInRequested" }
    function init() { notice.snapshot = null; notice.busy = false; notice.canReconnect = false; reconnects.clear(); signins.clear(); }
    function test_cold_offline_cache_requires_explicit_signin_and_never_reconnects_implicitly() {
        notice.snapshot = ({origin: "offline", message: "Offline cached libraries · incomplete"});
        verify(notice.visible);
        compare(reconnects.count, 0); compare(signins.count, 0);
        mouseClick(findChild(notice, "discoveryConnect"));
        compare(signins.count, 1); compare(reconnects.count, 0);
    }
    function test_partial_sources_offer_session_reconnect_and_busy_state_refuses_repeats() {
        notice.snapshot = ({origin: "partial", message: "One cached library"});
        notice.canReconnect = true;
        const button = findChild(notice, "discoveryConnect");
        button.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(reconnects.count, 1); compare(signins.count, 0);
        notice.busy = true; mouseClick(button); keyClick(Qt.Key_Space);
        compare(reconnects.count, 1); verify(!button.enabled);
        notice.snapshot = ({origin: "live"}); verify(!notice.visible);
    }
    function test_source_labels_distinguish_cached_availability_without_changing_identity() {
        picker.libraries = [{id: "cached", title: "Music", server: "Home", offline: true}, {id: "live", title: "Music", server: "Studio", offline: false}];
        picker.selectedId = "cached";
        compare(picker.caption(picker.selected), "Home · cached");
        compare(picker.selected.id, "cached");
        picker.selectedId = "live"; compare(picker.caption(picker.selected), "Studio");
    }
}
