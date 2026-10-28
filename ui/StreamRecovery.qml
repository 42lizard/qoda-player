import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: recovery
    required property var snapshot
    required property var queue
    required property bool connected
    required property bool canRetry
    readonly property bool exhausted: !!snapshot && snapshot.phase === "exhausted"
    visible: !!snapshot && ["recovering", "exhausted"].includes(snapshot.phase)
    signal actionRequested(string op, var fields)
    function action(op) {
        if (!queue || !queue.selected) return;
        actionRequested(op, {queue_session: queue.session, queue_revision: queue.revision, occurrence: queue.selected});
    }
    Label {
        text: recovery.snapshot ? I18n.message(recovery.snapshot.message) || "" : ""
        textFormat: Text.PlainText
        color: recovery.exhausted ? Theme.danger : Theme.muted
        wrapMode: Text.Wrap
        Layout.fillWidth: true
    }
    RowLayout {
        visible: recovery.exhausted
        Button {
            objectName: "retryStream"
            text: I18n.t("Retry stream")
            enabled: recovery.connected && recovery.canRetry && !!recovery.queue && !!recovery.queue.selected
            onClicked: recovery.action("retry_stream")
        }
        Button {
            objectName: "skipStream"
            text: I18n.t("Skip")
            enabled: recovery.connected && !!recovery.queue && !!recovery.queue.selected
            onClicked: recovery.action("skip_stream")
        }
    }
}
