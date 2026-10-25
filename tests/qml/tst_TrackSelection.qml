import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    name: "TrackSelection"
    Qoda.TrackSelection { id: selection }
    function init() {
        selection.tracks = ["A", "B", "C", "D", "E"].map(key => ({reference: {source: "fixture", key: key}}));
        selection.revision = "account:9007199254740993";
        selection.clear();
    }
    function test_control_and_checkbox_toggle_keep_page_order() {
        selection.choose(3, Qt.NoModifier);
        selection.choose(1, Qt.ControlModifier);
        compare(selection.indexes, [1, 3]);
        compare(selection.references.map(track => track.key), ["B", "D"]);
        selection.toggle(1);
        compare(selection.indexes, [3]);
        selection.choose(3, Qt.ControlModifier);
        compare(selection.count, 0);
    }
    function test_shift_ranges_and_control_shift_preserve_anchor() {
        selection.choose(3, Qt.NoModifier);
        selection.choose(1, Qt.ShiftModifier);
        compare(selection.indexes, [1, 2, 3]);
        selection.choose(4, Qt.ShiftModifier);
        compare(selection.indexes, [3, 4]);
        selection.choose(0, Qt.ControlModifier);
        selection.choose(1, Qt.ControlModifier | Qt.ShiftModifier);
        compare(selection.indexes, [0, 1, 3, 4]);
    }
    function test_account_page_revision_and_model_changes_clear_old_indexes() {
        selection.selectAll();
        compare(selection.count, 5);
        selection.revision = "account:9007199254740994";
        compare(selection.count, 0);
        compare(selection.anchor, -1);
        selection.toggle(2);
        selection.tracks = [{reference: {source: "other", key: "Z"}}];
        compare(selection.count, 0);
        selection.selectAll();
        compare(selection.references[0].key, "Z");
    }
    function test_empty_out_of_range_and_clear_never_invent_references() {
        selection.choose(-1, Qt.NoModifier); selection.toggle(99);
        compare(selection.count, 0);
        selection.selectAll(); selection.clear();
        compare(selection.references, []);
        selection.tracks = [];
        selection.selectAll(); selection.choose(0, Qt.ShiftModifier);
        compare(selection.count, 0);
        compare(selection.anchor, -1);
    }
}
