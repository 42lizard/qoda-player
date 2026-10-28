import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "GermanControls"
    width: 640; height: 620; visible: true; when: windowShown
    Qoda.PlayerTransport {
        id: transport
        width: test.width
        selectedTrack: ({title: "Play <Ä> %1", artist: "Next", album: "Album"})
        connected: true
        snapshot: ({intent: "paused", queue: {}, presentation: {known: false, can_play: true}, replaygain: {mode: "track", applied: {basis: "track", db: -3, reason: "ReplayGain off"}}})
    }
    Qoda.AudioSettings { id: audio; width: test.width; y: 200; connected: true; snapshot: null }
    SignalSpy { id: commands; target: transport; signalName: "transportRequested" }
    SignalSpy { id: output; target: audio; signalName: "outputRequested" }
    function init() { Qoda.I18n.preferences = ["de-AT"]; commands.clear(); output.clear(); }
    function cleanup() { Qoda.I18n.preferences = Qt.locale().uiLanguages; }
    function test_translated_controls_keep_identity_and_commands() {
        const play = findChild(transport, "playPause");
        compare(play.text, "Abspielen");
        compare(transport.track.title, "Play <Ä> %1");
        play.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(commands.count, 1); compare(commands.signalArguments[0][0], "resume");
        compare(findChild(audio, "applyOutput").text, "Ausgabe übernehmen");
        waitForPolish(transport);
        for (const name of ["previous", "playPause", "next", "replay", "seekPosition"]) {
            const item = findChild(transport, name);
            const point = item.mapToItem(transport, 0, 0);
            verify(point.x >= 0 && point.x + item.width <= transport.width, name + " is clipped");
        }
    }
}
