import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import "."

Rectangle {
    width: columnLayout.implicitWidth + 2
    height: columnLayout.implicitHeight + 2
    border.color: "#327eea"
    radius: Constants.defaultRadius
    border.width: 1
    color: "gray"
    clip: true


    ColumnLayout {
        id: columnLayout
        spacing: 0

        Text {
            id: title
            text: "<b>Node name</b>"
            Layout.alignment: Qt.AlignHCenter
        }

        RowLayout {
            spacing: Constants.defaultMargin

            Item {
                width: listView1.width + Constants.defaultMargin
                height: listView1.height + Constants.defaultMargin
                Layout.alignment: Qt.AlignTop | Qt.AlignLeft

                ListView {
                    id: listView1
                    height: Math.min(50, contentHeight)
                    width: 70
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.horizontalCenter: parent.horizontalCenter

                    model: ListModel {
                        ListElement { name: "Alice" }
                        ListElement { name: "Bob" }
                        ListElement { name: "Charlie" }
                    }

                    delegate: RowLayout {

                        Rectangle {
                            width: 10
                            height: 10
                            color: "orange"
                            radius: width / 2
                            border.color: "black"
                            border.width: 1
                        }

                        Text {
                            text: model.name
                        }
                    }

                }

            }


            Item {
                width: listView2.width + Constants.defaultMargin
                height: listView2.height + Constants.defaultMargin
                Layout.alignment: Qt.AlignTop | Qt.AlignRight

                ListView {
                    id: listView2
                    height: Math.min(50, contentHeight)
                    width: 70
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.horizontalCenter: parent.horizontalCenter

                    model: ListModel {
                        ListElement { name: "Alice" }
                        ListElement { name: "Bob" }
                        ListElement { name: "Charlie" }
                    }

                    delegate: RowLayout {
                        anchors.right: parent.right

                        Text {
                            text: model.name
                        }

                        Rectangle {
                            width: 10
                            height: 10
                            color: "orange"
                            radius: width / 2
                            border.color: "black"
                            border.width: 1
                        }
                    }
                }
            }
        }
    }
}
