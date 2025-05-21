import QtQuick
import com.cssodessa.AppController


Window {
    id: root
    width: 640
    height: 480
    visible: true
    title: qsTr("Hello World")
    color: "#1e1e1e"

    onAfterSynchronizing: {
        AppController.afterSynchronizing()
        allConnectionCanvas.repaintConnections()
        newConnectionCanvas.repaintConnections()
    }

    ConnectionsCanvas {
        id: allConnectionCanvas
        anchors.fill: parent

        appController: AppController
    }

    Repeater {
        model: AppController.nodes
        anchors.fill: parent

        delegate: Node {
            nodeController: modelData

            onViewPosChanged: {
                allConnectionCanvas.repaintConnections()
                newConnectionCanvas.repaintConnections()
            }
        }
    }

    NewConnectionCanvas {
        id: newConnectionCanvas
        anchors.fill: parent

        appController: AppController
    }
}

