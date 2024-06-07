import QtQuick
import com.cssodessa.AppController


Window {
    width: 640
    height: 480
    visible: true
    title: qsTr("Hello World")

    Rectangle {
        width: 100
        height: 100
        color: "lightgray"
        anchors.fill: parent

        Column {
            anchors.fill: parent
            spacing: 10

            Repeater {
                model: AppController.nodes
                delegate:
                    Rectangle {
                        width: 100
                        height: 100
                        color: "lightblue"
                        border.color: "blue"
                        border.width: 2
                        radius: 10

                        Text {
                            text: modelData.name
                            font.pointSize: 20
                        }
                    }
            }
        }

    }
}

