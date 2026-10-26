import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    name: "ReorderAuthority"
    Qoda.ReorderState { id: state; kind: "queue"; account: "4"; instance: "instance-1"; fence: ({}); rows: []; enabled: true }
    SignalSpy { id: requests; target: state; signalName: "requested" }
    function init() {
        state.cancel(); state.kind = "queue"; state.account = "4"; state.instance = "instance-1"; state.enabled = true;
        state.fence = {queue_session: "session", queue_revision: "9007199254740993"};
        state.rows = [{id: "selected", can_move: false}, {id: "duplicate-A", can_move: true}, {id: "B", can_move: true}, {id: "duplicate-B", can_move: true}];
        requests.clear();
    }
    function test_queue_drop_captures_exact_fences_and_independent_occurrences_without_optimistic_mutation() {
        const before = JSON.stringify(state.rows);
        verify(!state.begin("selected")); verify(state.begin("duplicate-A"));
        verify(state.propose("B", true)); compare(requests.count, 0);
        compare(JSON.stringify(state.rows), before);
        verify(state.finish(true, "duplicate-A")); compare(requests.count, 1);
        compare(requests.signalArguments[0][0], "queue_move");
        const fields = requests.signalArguments[0][1];
        compare(fields.occurrence, "duplicate-A"); compare(fields.before, "duplicate-B");
        compare(fields.queue_revision, "9007199254740993"); compare(fields.queue_session, "session"); compare(fields.account_epoch, "4");
        verify(!state.finish(true, "duplicate-A")); compare(requests.count, 1);
        compare(JSON.stringify(state.rows), before);
    }
    function test_first_last_noop_and_restricted_destinations_use_identity() {
        verify(state.begin("duplicate-B")); verify(state.propose("duplicate-A", false)); verify(state.finish(true));
        compare(requests.signalArguments[0][1].before, "duplicate-A");
        verify(state.begin("duplicate-A")); verify(state.propose(null, true)); verify(state.finish(true));
        compare(requests.signalArguments[1][1].before, null);
        verify(state.begin("duplicate-A")); verify(!state.propose("B", false)); verify(!state.propose("selected", false));
        verify(!state.propose("duplicate-A", true)); verify(!state.propose("missing", true));
        verify(!state.finish(true)); compare(requests.count, 2);
    }
    function test_snapshot_account_eligibility_and_disabled_changes_cancel_before_dispatch() {
        verify(state.begin("duplicate-A")); verify(state.propose("B", true));
        state.fence = {queue_session: "session", queue_revision: "9007199254740994"};
        verify(!state.active); verify(!state.finish(true));
        verify(state.begin("duplicate-A")); state.account = "5"; verify(!state.active);
        verify(state.begin("duplicate-A")); state.instance = "new-instance"; verify(!state.active);
        verify(state.begin("duplicate-A"));
        state.rows = [{id: "duplicate-A", can_move: false}, {id: "B", can_move: true}]; verify(!state.active);
        state.rows = [{id: "A", can_move: true}]; verify(state.begin("A")); state.enabled = false; verify(!state.active);
        compare(requests.count, 0);
    }
    function test_playlist_before_after_and_readonly_restrictions_preserve_item_ids() {
        state.kind = "playlist";
        state.fence = {reference: {source: "source", key: "playlist"}, expected_revision: "revision-1"};
        state.rows = [{id: "item-1", key: "42"}, {id: "item-2", key: "42"}, {id: "item-3", key: "43"}];
        verify(state.begin("item-1")); verify(state.propose("item-2", true)); verify(state.finish(true));
        compare(requests.signalArguments[0][0], "playlist_edit");
        const fields = requests.signalArguments[0][1];
        compare(fields.edit.item_id, "item-1"); compare(fields.edit.after, "item-2"); compare(fields.expected_revision, "revision-1");
        compare(fields.reference.source, "source"); compare(fields.reference.key, "playlist");
        verify(state.begin("item-3")); verify(state.propose("item-1", false)); verify(state.finish(true));
        compare(requests.signalArguments[1][1].edit.after, null);
        state.enabled = false; verify(!state.begin("item-1")); compare(requests.count, 2);
    }
    function test_old_handlers_cannot_adopt_a_new_gesture_for_the_same_item() {
        verify(state.begin("duplicate-A")); const old = state.generation;
        state.cancel(); verify(state.begin("duplicate-A"));
        verify(state.propose("B", true));
        state.cancel("duplicate-A", old); verify(state.active);
        verify(!state.finish(true, "duplicate-A", old)); compare(requests.count, 0);
        verify(state.finish(true, "duplicate-A", state.generation)); compare(requests.count, 1);
    }
    function test_unaccepted_cancelled_ambiguous_or_superseded_drags_do_not_emit() {
        verify(state.begin("duplicate-A")); verify(state.propose("B", true)); verify(!state.finish(false)); compare(requests.count, 0);
        verify(state.begin("duplicate-A")); state.cancel("another-item"); verify(state.active); state.cancel(); verify(!state.active);
        state.rows = [{id: "A", can_move: true}, {id: "A", can_move: true}]; verify(!state.begin("A"));
        compare(requests.count, 0);
    }
}
