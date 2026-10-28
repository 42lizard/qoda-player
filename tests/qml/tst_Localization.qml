import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    name: "Localization"
    function cleanup() { Qoda.I18n.preferences = Qt.locale().uiLanguages; }
    function test_locale_fallback_and_regional_german() {
        for (const name of ["de-DE", "de_AT.UTF-8", "de-CH"]) {
            Qoda.I18n.preferences = [name];
            compare(Qoda.I18n.t("Play"), "Abspielen");
        }
        for (const names of [["en-US"], ["fr-FR"], ["C"], ["POSIX"], []]) {
            Qoda.I18n.preferences = names;
            compare(Qoda.I18n.t("Play"), "Play");
        }
        Qoda.I18n.preferences = ["fr-FR", "de-DE", "en-US"];
        compare(Qoda.I18n.t("Play"), "Abspielen");
    }
    function test_parameters_plurals_and_unknown_fallback() {
        Qoda.I18n.preferences = ["en-US"];
        compare(Qoda.I18n.count("%1 track", "%1 tracks", "1"), "1 track");
        compare(Qoda.I18n.count("%1 track", "%1 tracks", "2"), "2 tracks");
        Qoda.I18n.preferences = ["de-DE"];
        compare(Qoda.I18n.t("Future message"), "Future message");
        compare(Qoda.I18n.t("%1 track", ["A {x} $1 %2 & Ä"]), "A {x} $1 %2 & Ä Titel");
        compare(Qoda.I18n.count("%1 track", "%1 tracks", 1), "1 Titel");
        compare(Qoda.I18n.count("%1 track", "%1 tracks", 2), "2 Titel");
    }
    function test_backend_messages_keep_parameters_and_unknown_details() {
        Qoda.I18n.preferences = ["de-DE"];
        compare(Qoda.I18n.message("Plex sign-in is required."), "Eine Plex-Anmeldung ist erforderlich.");
        compare(Qoda.I18n.message("Plex sign-in is required. Recovery stopped. Retry or Skip explicitly."), "Eine Plex-Anmeldung ist erforderlich. Wiederherstellung gestoppt. Wähle „Erneut versuchen“ oder „Überspringen“.");
        compare(Qoda.I18n.message("Playback USB Ä / $1 %2"), "Playback USB Ä / $1 %2");
        compare(Qoda.I18n.message("Unknown future backend failure: <Music>"), "Unknown future backend failure: <Music>");
        compare(Qoda.I18n.message(null), "");
    }
    Qoda.BackendState { id: session }
    function test_localization_does_not_modify_command_fences() {
        session.snapshot = {instance: "fixture", account_epoch: "9007199254740993", queue: {session: "session-Ä", revision: "9007199254740995"}};
        for (const language of ["en-US", "de-DE"]) {
            Qoda.I18n.preferences = [language];
            const request = session.request("queue_play", {occurrence: "track-ß"}, true);
            compare(request.op, "queue_play"); compare(request.occurrence, "track-ß");
            compare(request.account_epoch, "9007199254740993");
            compare(request.queue_revision, "9007199254740995"); compare(request.queue_session, "session-Ä");
        }
    }
    function test_dynamic_backend_templates_are_anchored_and_keep_names() {
        Qoda.I18n.preferences = ["de-DE"];
        compare(Qoda.I18n.message("Saved session cleanup is incomplete. Plex sign-in is required."), "Bereinigung der gespeicherten Sitzung unvollständig. Eine Plex-Anmeldung ist erforderlich.");
        compare(Qoda.I18n.message("Saved session cleanup is incompleteX Plex sign-in is required."), "Saved session cleanup is incompleteX Plex sign-in is required.");
        compare(Qoda.I18n.message("Nominal observations · 30 days · at most 10000 visits. Positions are not listening duration."), "Nominelle Beobachtungen · 30 Tage · höchstens 10000 Durchläufe. Positionen sind keine Hördauer.");
        compare(Qoda.I18n.message("2 matches · 1/3 libraries ready · partial results · limited scope; refine query"), "2 Treffer · 1/3 Mediatheken bereit · Teilergebnisse · begrenzter Umfang; Suche eingrenzen");
    }
}
