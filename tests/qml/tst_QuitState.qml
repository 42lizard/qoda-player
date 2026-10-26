import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    name: "QuitAcknowledgement"
    Qoda.QuitState { id: state; timeoutMs: 30 }
    SignalSpy { id: sent; target: state; signalName: "sendQuit" }
    SignalSpy { id: exited; target: state; signalName: "exitUi" }
    function init() {
        state.deadline.stop(); state.pending = false; state.error = "";
        state.connected = true; state.instance = "backend-a";
        sent.clear(); exited.clear();
    }
    function test_connected_quit_waits_for_ack_and_coalesces_duplicates() {
        state.begin(); state.begin();
        compare(sent.count, 1); compare(exited.count, 0); verify(state.pending);
        verify(!state.observe({instance:"backend-a", shutdown:false}));
        compare(exited.count, 0);
        verify(state.observe({instance:"backend-a", shutdown:true}));
        compare(exited.count, 1); verify(!state.pending);
    }
    function test_timeout_reports_error_instead_of_closing_early() {
        state.begin(); tryCompare(state, "pending", false);
        verify(state.error.length > 0); compare(exited.count, 0);
        state.begin(); compare(sent.count, 2); compare(state.error, "");
        state.observe({instance:"backend-a", shutdown:true});
    }
    function test_disconnected_or_replaced_owner_exits_without_quitting_another_backend() {
        state.connected = false; state.begin();
        compare(sent.count, 0); compare(exited.count, 1);
        init(); state.begin(); state.instance = "backend-b";
        verify(state.observe({instance:"backend-b", shutdown:false}));
        compare(sent.count, 1); compare(exited.count, 1);
    }
}
