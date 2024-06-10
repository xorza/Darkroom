import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import com.cssodessa.NodeController


Rectangle {
    property NodeController nodeController


    signal viewPosChanged()

    id: root

    color: "#2e2e2e"
    border.color: "#363636"
    border.width: 1
    radius: 3
    width: columnLayout.width
    height: columnLayout.height + 5
    x: nodeController.viewPos.x
    y: nodeController.viewPos.y

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true

        onClicked: {
            if (!nodeController.selected)
                nodeController.selected = true
        }
    }

    Component.onCompleted: {
        nodeController.item = root
    }

    ColumnLayout {
        id: columnLayout
        spacing: 5

        Rectangle {
            color: nodeController.selected ? "orange" : "#242424"
            height: 28
            Layout.fillWidth: true
            topRightRadius: 2
            topLeftRadius: 2
            Layout.margins: 1

            MouseArea {
                anchors.fill: parent
                pressAndHoldInterval: 10
                drag.target: root
                drag.axis: Drag.XAxis | Drag.YAxis

                onPositionChanged: {
                    if (drag.active) {
                        nodeController.viewPos = Qt.point(root.x, root.y)
                        viewPosChanged()
                    }
                }
                onClicked: {
                    if (!nodeController.selected) {
                        nodeController.selected = true
                    }
                }
            }

            Rectangle {
                id: triggerPin
                width: 10
                height: 10
                radius: 5
                color: triggerMouseArea.containsMouse
                    ? triggerMouseArea.containsPress
                        ? Qt.darker("yellow")
                        : Qt.lighter("yellow")
                    : "yellow"
                x: -5
                y: -5

                Component.onCompleted: {
                    nodeController.trigger.pin = triggerPin
                }

                MouseArea {
                    id: triggerMouseArea
                    anchors.fill: parent
                    hoverEnabled: true

                    onClicked: {
                        nodeController.trigger.selected = !nodeController.trigger.selected
                    }
                }
            }

            Text {
                color: nodeController.selected ? "black" : "lightgray"
                anchors {
                    verticalCenter: parent.verticalCenter
                    horizontalCenter: parent.horizontalCenter
                }
                text: root.nodeController ? root.nodeController.name : "no nodeController"
            }

            Rectangle {
                id: deleteButton
                width: 16
                height: 16
                radius: 5
                anchors {
                    verticalCenter: parent.verticalCenter
                    right: parent.right
                    rightMargin: 5
                }
                color: deleteMouseArea.containsMouse
                    ? deleteMouseArea.containsPress
                        ? Qt.darker("gray")
                        : Qt.lighter("gray")
                    : "gray"

                Rectangle {
                    width: 10
                    height: 1
                    color: "darkgray"
                    anchors.centerIn: parent
                    rotation: 45
                }
                Rectangle {
                    width: 10
                    height: 1
                    color: "darkgray"
                    anchors.centerIn: parent
                    rotation: -45
                }

                MouseArea {
                    id: deleteMouseArea
                    anchors.fill: parent
                    hoverEnabled: true

                    onClicked: {
                        nodeController.remove()
                    }
                }
            }
        }

        GridLayout {
            width: gridLayout.width
            columns: 3

            ColumnLayout {
                id: inputsColumn
                Layout.alignment: Qt.AlignTop
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.columnSpan: 1

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
                                color: inputMouseArea.containsMouse
                                    ? inputMouseArea.containsPress
                                        ? Qt.darker("red")
                                        : Qt.lighter("red")
                                    : "red"
                                radius: 5
                                anchors.verticalCenter: parent.verticalCenter

                                Component.onCompleted: {
                                    modelData.pin = inputPin
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
                            onClicked: {
                                modelData.selected = !modelData.selected
                            }
                        }
                    }
                }
            }

            Item {
                Layout.fillWidth: true
            }

            ColumnLayout {
                id: outputsColumn
                Layout.alignment: Qt.AlignTop | Qt.AlignRight
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.columnSpan: 1

                Repeater {
                    model: nodeController.outputs
                    Layout.alignment: Qt.AlignRight
                    Layout.fillWidth: true

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
                                color: outputMouseArea.containsMouse
                                    ? outputMouseArea.containsPress
                                        ? Qt.darker("red")
                                        : Qt.lighter("red")
                                    : "red"
                                radius: 5
                                anchors.verticalCenter: parent.verticalCenter

                                Component.onCompleted: {
                                    modelData.pin = outputPin
                                }
                            }
                        }
                        MouseArea {
                            id: outputMouseArea
                            anchors.fill: parent
                            hoverEnabled: true
                            onClicked: {
                                modelData.selected = !modelData.selected
                            }
                        }
                    }
                }

                Repeater {
                    model: nodeController.events

                    delegate: Item {
                        Layout.rightMargin: -5
                        Layout.alignment: Qt.AlignRight
                        width: eventRow.width
                        height: eventRow.height

                        Row {
                            id: eventRow
                            spacing: 5

                            Text {
                                text: modelData.name
                                anchors.verticalCenter: parent.verticalCenter
                                color: "darkgray"
                            }
                            Rectangle {
                                id: eventPin
                                width: 10
                                height: 10
                                color: eventMouseArea.containsMouse
                                    ? eventMouseArea.containsPress
                                        ? Qt.darker("yellow")
                                        : Qt.lighter("yellow")
                                    : "yellow"
                                radius: 5
                                anchors.verticalCenter: parent.verticalCenter

                                Component.onCompleted: {
                                    modelData.pin = eventPin
                                }
                            }
                        }
                        MouseArea {
                            id: eventMouseArea
                            anchors.fill: parent
                            hoverEnabled: true
                            onClicked: {
                                modelData.selected = !modelData.selected
                            }
                        }
                    }
                }
            }
        }

        GridLayout {
            id: gridLayout
            columns: 2

            ToggleButton {
                text: "cache results"
                Layout.fillWidth: true
                Layout.leftMargin: 5
            }
            ToggleButton {
                text: "output"
                Layout.fillWidth: true
                Layout.rightMargin: 5
            }

        }
    }
}

