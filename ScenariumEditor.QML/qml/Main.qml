import QtQuick
import com.cssodessa.AppController


Window {
    width: 640
    height: 480
    visible: true
    title: qsTr("Hello World")
    color: "#1e1e1e"

    Column {
        anchors.fill: parent
        spacing: 10
        anchors.margins: 10

        Repeater {
            model: AppController.nodes

            delegate: Node {
                nodeController: modelData
            }
        }
    }

}

