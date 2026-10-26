import QtQuick
import QtQuick.Controls
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "Theme"
    width: 600; height: 120; visible: true; when: windowShown
    Qoda.ThemedControl {
        id: controls
        Qoda.TrackRow { id: row; width: 600; track: ({title: "Synthetic", artist: "Fixture", album: "Album"}); index: 0; selected: false }
    }
    function cleanup() { Qoda.Theme.colors = ({}); }
    function resolved(background, foreground, accent) { return "background\t" + background + "\nforeground\t" + foreground + "\naccent\t" + accent + "\nurgent\t#ee4455\n"; }
    function test_dark_and_light_palettes_update_existing_rows_and_controls() {
        for (const data of [["#202830", "#efe0cc", "#69c9ba"], ["#fff8ec", "#222222", "#992244"]]) {
            verify(Qoda.Theme.apply(resolved(...data)));
            compare(String(Qoda.Theme.background), data[0]);
            tryVerify(() => String(controls.palette.text) === data[1]);
            tryVerify(() => String(controls.palette.highlight) === data[2]);
            tryVerify(() => String(findChild(row, "playNow").palette.text) === data[1]);
            compare(row.color, Qoda.Theme.surface);
            row.selected = true; compare(row.color, Qoda.Theme.selected);
            row.selected = false;
            verify(Qoda.Theme.contrast(Qoda.Theme.accent, Qoda.Theme.accentText) >= 4.5);
            compare(String(Qoda.Theme.danger), "#ee4455");
        }
    }
    function test_invalid_partial_and_oversized_updates_retain_last_valid_palette() {
        verify(Qoda.Theme.apply(resolved("#123456", "#ffffff", "#aabbcc")));
        for (const invalid of ["", "background\t#000000", "background\tnot-a-color\nforeground\t#ffffff\naccent\t#aaaaaa", "x".repeat(65537)]) {
            verify(!Qoda.Theme.apply(invalid));
            compare(String(Qoda.Theme.background), "#123456");
        }
    }
    function test_missing_omarchy_uses_the_approved_fallback() {
        Qoda.Theme.colors = ({});
        verify(!Qoda.Theme.system);
        compare(String(Qoda.Theme.background), "#111513");
        compare(String(Qoda.Theme.foreground), "#e8eee8");
        compare(String(Qoda.Theme.accent), "#c6eb9d");
    }
}
