import QtQuick

DropArea {
    id: zone
    required property var controller
    required property string targetId
    property bool movable: true
    property bool after: false
    property bool append: false
    keys: controller ? [controller.scope] : []
    enabled: movable && !!controller && controller.enabled
    function accepts(event) {
        return controller.current() && !!event.source && event.source.controller === controller && event.source.itemId === controller.itemId && event.source.generation === controller.generation && (append || targetId !== controller.itemId);
    }
    onEntered: event => { event.accepted = accepts(event); after = event.y >= height / 2; }
    onPositionChanged: event => { event.accepted = accepts(event); after = event.y >= height / 2; }
    onDropped: event => {
        if (accepts(event) && controller.propose(append ? null : targetId, event.y >= height / 2)) event.acceptProposedAction();
        else event.accepted = false;
    }
    Rectangle {
        width: parent.width; height: 2; color: Theme.accent
        y: zone.after ? parent.height - height : 0
        visible: zone.containsDrag && zone.controller.active
    }
}
