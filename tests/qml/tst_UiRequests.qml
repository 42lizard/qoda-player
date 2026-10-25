import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    name: "DesktopRaiseRequests"
    Qoda.UiRequests { id: requests }
    function init() { requests.reset(); }
    function test_first_snapshot_and_reconnect_do_not_replay_old_raise() {
        verify(!requests.consume("backend", "old"));
        verify(!requests.consume("backend", "old"));
        verify(requests.consume("backend", "new"));
        verify(!requests.consume("backend", "new"));
        requests.reset(); verify(!requests.consume("backend", "new"));
    }
    function test_new_backend_and_missing_tokens_do_not_show_a_hidden_ui() {
        verify(!requests.consume("first", null));
        verify(requests.consume("first", "one"));
        verify(!requests.consume("first", null));
        verify(!requests.consume("first", "one"));
        verify(!requests.consume("second", "two"));
        verify(requests.consume("second", "three"));
    }
}
