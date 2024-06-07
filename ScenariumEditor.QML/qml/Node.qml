import QtQuick
import QtQuick.Layouts
import com.cssodessa.NodeController


Rectangle {
    property NodeController nodeController

    id: root

    color: "#2e2e2e"
    border.color: "#363636"
    border.width: 1
    radius: 3
    width: columnLayout.width
    height: columnLayout.height + 10
    x: nodeController.viewPos.x
    y: nodeController.viewPos.y

    MouseArea {
        anchors.fill: parent
    }

    ColumnLayout {
        id: columnLayout

        ColumnLayout {
            Layout.margins: 1
            Layout.fillWidth: true

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

                MouseArea {
                    anchors.fill: parent
                    pressAndHoldInterval: 10
                    drag.target: root
                    drag.axis: Drag.XAxis | Drag.YAxis

                    onPressed: {
                    }
                    onReleased: {
                        nodeController.viewPos = Qt.point(root.x, root.y)
                    }
                }
            }

            RowLayout {
                spacing: 10

                ColumnLayout {
                    id: inputsColumn
                    Layout.alignment: Qt.AlignTop

                    Repeater {
                        model: nodeController.inputs

                        delegate: Item {
                            width: inputRow.width
                            height: inputRow.height
                            Layout.leftMargin: -5

                            Row {
                                id: inputRow
                                spacing: 5

                                Rectangle {
                                    id: inputPin
                                    width: 10
                                    height: 10
                                    color: inputMouseArea.containsMouse ? inputMouseArea.containsPress ? Qt.darker("red") : Qt.lighter("red") : "red"
                                    radius: 5
                                    anchors.verticalCenter: parent.verticalCenter

                                    Component.onCompleted: {
                                        const point = inputPin.mapToItem(root, 0, 0);
                                        modelData.viewPos = point
                                    }
                                }
                                Text {
                                    text: modelData.name
                                    anchors.verticalCenter: parent.verticalCenter
                                    color: "darkgray"
                                }
                            }
                            MouseArea {
                                id: inputMouseArea
                                anchors.fill: parent
                                hoverEnabled: true
                            }
                        }
                    }
                }


                ColumnLayout {
                    id: outputsColumn
                    Layout.alignment: Qt.AlignTop

                    Repeater {
                        model: nodeController.outputs

                        delegate: Item {
                            Layout.rightMargin: -5
                            Layout.alignment: Qt.AlignRight
                            width: outputRow.width
                            height: outputRow.height

                            Row {
                                id: outputRow
                                spacing: 5

                                Text {
                                    text: modelData.name
                                    anchors.verticalCenter: parent.verticalCenter
                                    color: "darkgray"
                                }
                                Rectangle {
                                    id: outputPin
                                    width: 10
                                    height: 10
                                    color: outputMouseArea.containsMouse ? outputMouseArea.containsPress ? Qt.darker("red") : Qt.lighter("red") : "red"
                                    radius: 5
                                    anchors.verticalCenter: parent.verticalCenter

                                    Component.onCompleted: {
                                        const point = outputPin.mapToItem(root, 0, 0);
                                        modelData.viewPos = point
                                    }
                                }
                            }
                            MouseArea {
                                id: outputMouseArea
                                anchors.fill: parent
                                hoverEnabled: true
                            }
                        }
                    }
                }
            }
        }
    }
}

