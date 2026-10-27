import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    name: "WidgetState"
    Qoda.WidgetState { id: state }
    function envelope(instance, revision) {
        return {protocol: 1, instance: instance, revision: revision, account_epoch: "9007199254740993", queue: {session: "session", revision: "18446744073709551614", entries: [{selected: true, track: {title: "Selected"}}]}};
    }
    function init() { state.reset(); }
    function test_large_counters_are_preserved_and_stale_frames_cannot_replace_current_state() {
        verify(state.accept(envelope("A", "9007199254740993")));
        verify(state.accept(envelope("A", "9007199254740992")));
        compare(state.snapshot.revision, "9007199254740993");
        const request = state.request("next");
        compare(request.instance, "A"); compare(request.account_epoch, "9007199254740993");
        compare(request.queue_revision, "18446744073709551614"); compare(request.queue_session, "session");
        compare(state.selectedTrack.title, "Selected");
        verify(state.accept(envelope("B", "0")));
        compare(state.request("next").instance, "B");
    }
    function test_shared_envelope_copies_fields_and_preserves_queue_fences() {
        verify(state.accept(envelope("A", "2")));
        const fields = {instance: "old", account_epoch: "0", queue_revision: "0", value: 42};
        const first = state.request("next", fields);
        compare(first.value, 42); compare(first.instance, "A");
        compare(first.queue_revision, "18446744073709551614");
        compare(fields.instance, "old"); compare(fields.queue_revision, "0");
        verify(first.id !== state.request("next").id);
    }
    function test_playing_indicator_requires_confirmed_backend_status() {
        let value = envelope("A", "1");
        value.intent = "playing"; value.presentation = {known: false, status: "Playing"};
        state.accept(value); verify(!state.playing);
        value = envelope("A", "2"); value.intent = "playing"; value.presentation = {known: true, status: "Paused"};
        state.accept(value); verify(!state.playing);
        value = envelope("A", "3"); value.presentation = {known: true, status: "Playing"};
        state.accept(value); verify(state.playing);
        state.reset(); verify(!state.playing);
    }
    function test_disconnect_and_invalid_protocol_or_counters_do_not_grant_commands() {
        compare(state.request("resume"), null);
        for (const value of [1, "01", "-1", "18446744073709551616", "1e2"]) {
            verify(!state.accept(envelope("A", value)));
            compare(state.request("next"), null);
        }
        let next = envelope("A", "1"); next.protocol = 2;
        verify(!state.accept(next));
        verify(state.accept(envelope("A", "1")));
        compare(state.request("seek"), null); compare(state.request("auth_start"), null);
        state.reset(); compare(state.request("next"), null); compare(state.selectedTrack, null);
    }
}
