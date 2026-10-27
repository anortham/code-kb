import QtQuick
Item {
    function anchor_qml(value) { return value + 41 }
    function caller_qml() { return anchor_qml(1) }
}
