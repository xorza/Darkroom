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
        connectionCanvas.repaintConnections()
    }

    ConnectionsCanvas {
        id: connectionCanvas
        anchors.fill: parent
        anchors.margins: 10

        appController: AppController
    }

    Item {
        anchors.fill: parent
        anchors.margins: 10

        Repeater {
            model: AppController.nodes

            delegate: Node {
                nodeController: modelData

                onViewPosChanged: {
                    // connectionCanvas.requestPaint()
                    connectionCanvas.repaintConnections()
                }
            }
        }


    }

}

