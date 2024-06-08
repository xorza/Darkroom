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
        connectionCanvas.requestPaint()
    }


    Canvas {
        id: connectionCanvas

        anchors.fill: parent
        anchors.margins: 10
        contextType: "2d"

        onPaint: {
            context.save()
            context.clearRect(0, 0, width, height)

            context.strokeStyle = "green"
            context.lineWidth = 3


            for (var i = 0; i < AppController.connections.length; i++) {
                var connection = AppController.connections[i]
                var output = connection.source.outputs[connection.outputIdx]
                var input = connection.target.inputs[connection.inputIdx]

                context.beginPath()
                context.moveTo(output.viewPos.x, output.viewPos.y)
                context.bezierCurveTo(
                    output.viewPos.x + 40, output.viewPos.y,
                    input.viewPos.x - 40, input.viewPos.y,
                    input.viewPos.x, input.viewPos.y
                )
                context.stroke()
            }

            context.restore()
        }
    }

    Item {
        anchors.fill: parent
        anchors.margins: 10

        Repeater {
            model: AppController.nodes

            delegate: Node {
                nodeController: modelData

                onViewPosChanged: {
                    connectionCanvas.requestPaint()
                }
            }
        }


    }

}

