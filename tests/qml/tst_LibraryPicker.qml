import QtQuick
import QtQuick.Controls
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "LibraryPicker"
    width: 540; height: 400; visible: true; when: windowShown
    Qoda.LibraryPicker { id: picker; width: 88; libraries: []; selectedId: "" }
    SignalSpy { id: choices; target: picker; signalName: "chosen" }
    SignalSpy { id: searches; target: picker; signalName: "searchRequested" }
    function init() {
        picker.libraries = [{id: "home", title: "Music", server: "Home"}, {id: "sameprefix-A", title: "Music", server: "Studio"}, {id: "sameprefix-B", title: "Music", server: "Studio"}];
        picker.selectedId = "home"; choices.clear(); searches.clear();
    }
    function cleanup() { picker.dismiss(); tryCompare(picker, "popupVisible", false); }
    function test_duplicate_names_are_source_labelled_and_do_not_guess_selection() {
        verify(picker.label(picker.libraries[0]).includes("Home"));
        verify(picker.label(picker.libraries[1]).includes("Studio"));
        verify(picker.label(picker.libraries[1]) !== picker.label(picker.libraries[2]));
        picker.selectedId = "removed";
        compare(findChild(picker, "libraryCombo").currentIndex, -1);
        compare(choices.count, 0);
        picker.libraries = [];
        verify(!findChild(picker, "libraryCombo").enabled);
    }
    function test_keyboard_choice_emits_source_identity_without_changing_authoritative_selection() {
        const combo = findChild(picker, "libraryCombo");
        combo.forceActiveFocus(); keyClick(Qt.Key_Down);
        compare(choices.count, 1); compare(choices.signalArguments[0][0], "sameprefix-A");
        compare(picker.selectedId, "home");
        picker.selectedId = "sameprefix-A";
        picker.libraries = picker.libraries.slice().reverse();
        compare(combo.currentIndex, 1); compare(choices.count, 1);
    }
    function test_popup_fits_narrow_rail_and_model_replacement_closes_stale_choices() {
        const combo = findChild(picker, "libraryCombo");
        mouseClick(combo, combo.width / 2, combo.height / 2);
        tryCompare(combo.popup, "opened", true);
        verify(combo.popup.width > picker.width && combo.popup.width <= test.width);
        picker.libraries = [{id: "new", title: "Other", server: "Elsewhere"}];
        tryCompare(picker, "popupVisible", false); compare(choices.count, 0);
    }
    function test_pointer_selects_the_disambiguated_source_id() {
        const combo = findChild(picker, "libraryCombo");
        mouseClick(combo, combo.width / 2, combo.height / 2); tryCompare(combo.popup, "opened", true);
        const view = findChild(combo.popup.contentItem, "sourceChoices");
        waitForPolish(view); view.forceLayout();
        const row = view.itemAtIndex(2); verify(!!row);
        mouseClick(row, row.width / 2, row.height / 2);
        compare(choices.count, 1); compare(choices.signalArguments[0][0], "sameprefix-B");
    }
    function test_escape_closes_choices_without_hiding_player_or_selecting() {
        const combo = findChild(picker, "libraryCombo");
        mouseClick(combo, combo.width / 2, combo.height / 2); tryCompare(combo.popup, "opened", true);
        keyClick(Qt.Key_Escape); tryCompare(picker, "popupVisible", false);
        verify(test.visible); compare(picker.selectedId, "home"); compare(choices.count, 0);
    }
    function test_search_shortcut_dismisses_the_popup() {
        const combo = findChild(picker, "libraryCombo");
        mouseClick(combo, combo.width / 2, combo.height / 2); tryCompare(combo.popup, "opened", true);
        keyClick(Qt.Key_K, Qt.ControlModifier);
        compare(searches.count, 1); tryCompare(picker, "popupVisible", false); compare(choices.count, 0);
    }
}
