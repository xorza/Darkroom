import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import "."

Rectangle {
    color: Constants.backgroundColor

    RowLayout {
         anchors.fill: parent

         ListView {
            model: app_model.functions
            Layout.preferredWidth: 200
            Layout.fillHeight: true
            Layout.alignment: Qt.AlignTop | Qt.AlignLeft

            delegate: Item {
                width: parent.width + Constants.defaultMargin
                implicitHeight: rowLayout.implicitHeight + Constants.defaultMargin

                Rectangle {
                    width: parent.width - Constants.defaultMargin
                    implicitHeight: rowLayout.implicitHeight
                    anchors.centerIn: parent
                    color: "blue"

                    RowLayout {
                        id: rowLayout
                        width: parent.width

                        Text {
                            text: modelData.name
                            Layout.alignment: Qt.AlignVCenter | Qt.AlignLeft
                        }
                        Button {
                            text: "+"
                            Layout.alignment: Qt.AlignVCenter | Qt.AlignRight
                            onClicked: {

                            }
                        }
                    }
                }
            }
         }

        ColumnLayout {
            Layout.fillWidth: false
            Layout.fillHeight: true
            Layout.alignment: Qt.AlignTop | Qt.AlignLeft

            ListView {
                Layout.preferredWidth: 200
                height: contentHeight
                model: app_model.functions
                Layout.alignment: Qt.AlignTop | Qt.AlignLeft

                delegate: Item {
                    width: functionNode.width + Constants.defaultMargin
                    height: functionNode.height + Constants.defaultMargin

                    Node {
                        id: functionNode
                        anchors.centerIn: parent
                        func: modelData
                    }
                }
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
        }
    }
}

