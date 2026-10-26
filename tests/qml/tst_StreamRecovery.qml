import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    name: "StreamRecovery"
    width: 560; height: 200; visible: true; when: windowShown
    Qoda.StreamRecovery {
        id: recovery
        anchors.fill: parent
        snapshot: null; queue: null; connected: true; canRetry: true
    }
    SignalSpy { id: actions; target: recovery; signalName: "actionRequested" }
    function init() {
        recovery.connected = true; recovery.canRetry = true;
        recovery.queue = {session: "fixture-session", revision: "9007199254740993", selected: "occurrence"};
        recovery.snapshot = {phase: "exhausted", attempts: 2, limit: 2, message: "Recovery stopped; retry explicitly"};
        actions.clear(); waitForPolish(recovery);
    }
    function test_recovery_is_informational_until_exhausted() {
        recovery.snapshot = {phase: "recovering", message: "Recovering pinned stream"};
        verify(recovery.visible); verify(!findChild(recovery, "retryStream").visible);
        compare(actions.count, 0);
        recovery.snapshot = {phase: "ready", message: "Refreshed"}; verify(!recovery.visible);
    }
    function test_retry_and_skip_carry_lossless_exact_occurrence_fences() {
        const retry = findChild(recovery, "retryStream");
        retry.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(actions.count, 1); compare(actions.signalArguments[0][0], "retry_stream");
        compare(actions.signalArguments[0][1].queue_revision, "9007199254740993");
        compare(actions.signalArguments[0][1].queue_session, "fixture-session");
        compare(actions.signalArguments[0][1].occurrence, "occurrence");
        const skip = findChild(recovery, "skipStream");
        skip.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(actions.count, 2); compare(actions.signalArguments[1][0], "skip_stream");
    }
    function test_offline_allows_explicit_skip_but_retry_requires_provider() {
        recovery.canRetry = false;
        verify(!findChild(recovery, "retryStream").enabled);
        verify(findChild(recovery, "skipStream").enabled);
        recovery.connected = false; verify(!findChild(recovery, "skipStream").enabled);
        compare(actions.count, 0);
    }
}
