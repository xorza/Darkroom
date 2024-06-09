import QtQuick
import QtQuick.Layouts
import QtQuick.Controls

Rectangle {
    id: root
    property bool pressed: false
    property alias text: textLabel.text

    color: pressed ? "orange" : "#242424"

    radius: 3
    border.color: "#363636"
    border.width: 1
    height: rowLayout.height
    width: rowLayout.width


    RowLayout {
        id: rowLayout
        Text {
            Layout.margins: 5
            Layout.alignment: Qt.AlignHCenter | Qt.AlignVCenter
            id: textLabel
            text: root.text
            color: pressed ? "black" : "darkgray"
        }
    }
    MouseArea {
        id: mouseArea
        anchors.fill: parent
        onClicked: {
            root.pressed = !root.pressed
        }
    }

}