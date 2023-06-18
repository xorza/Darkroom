import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    width: 500
    height: 500

    Rectangle {
        width: 300
        height: 300
        color: "red"
        radius: 40
        border.color: "black"
        border.width: 5
        clip:true

        Rectangle {
            id: innerRect
            anchors.fill: parent
            color: "blue"
        }
    }
}
