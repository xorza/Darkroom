import QtQuick
import QtQuick.Layouts
import com.cssodessa.NodeController


ColumnLayout {
    property NodeController nodeController

    id: root

    Rectangle {
        anchors.fill: parent
        width: 140
        color: "#2e2e2e"
        border.color: "#363636"
        border.width: 1
        radius: 3
        height: 50
    }

    ColumnLayout {
        id: column1

        Layout.topMargin: 5
        Layout.bottomMargin: 0
        anchors.fill: parent

        width: parent.width - 2
        y: 1
        anchors {
            horizontalCenter: parent.horizontalCenter
        }

        Rectangle {
            color: "#242424"
            height: 28
            Layout.fillWidth: true
            topRightRadius: 2
            topLeftRadius: 2
            Layout.margins: 1

            Text {
                color: "lightgray"
                anchors {
                    verticalCenter: parent.verticalCenter
                    horizontalCenter: parent.horizontalCenter
                }
                text: root.nodeController ? root.nodeController.name : "no nodeController"
            }
        }

        RowLayout {
            spacing: 10

            ColumnLayout {
                id: inputsColumn
                Layout.alignment: Qt.AlignTop

                Repeater {
                    model: nodeController.inputs

                    delegate: Row {
                        Layout.leftMargin: -5
                        spacing: 5

                        Rectangle {
                            width: 10
                            height: 10
                            color: "red"
                            radius: 5
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        Text {
                            text: modelData.name
                            anchors.verticalCenter: parent.verticalCenter
                            color: "darkgray"
                        }
                    }
                }
            }

            ColumnLayout {
                id: outputsColumn
                Layout.alignment: Qt.AlignTop

                Repeater {
                    model: nodeController.outputs

                    delegate: Row {
                        Layout.rightMargin: -5
                        Layout.alignment: Qt.AlignRight
                        spacing: 5

                        Text {
                            text: modelData.name
                            anchors.verticalCenter: parent.verticalCenter
                            color: "darkgray"
                        }
                        Rectangle {
                            width: 10
                            height: 10
                            color: "red"
                            radius: 5
                            anchors.verticalCenter: parent.verticalCenter
                        }
                    }
                }
            }
        }
    }
}
