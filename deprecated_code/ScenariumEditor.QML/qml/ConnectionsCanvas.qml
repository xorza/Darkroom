import QtQuick
import com.cssodessa.AppController
import com.cssodessa.NodeController
import com.cssodessa.ConnectionController
import com.cssodessa.ArgumentController


Canvas {
    id: canvas

    property AppController appController

    function repaintConnections() {
        canvas.requestPaint()
    }

    function drawAllConnections() {
        for (let i = 0; i < appController.connections.length; i++) {
            const connection = appController.connections[i];
            let input;
            let output;
            if (connection.connectionType === ConnectionController.ConnectionType.Data) {
                output = connection.source.outputs[connection.outputIdx];
                input = connection.target.inputs[connection.inputIdx];
                context.strokeStyle = "red"
            } else {
                output = connection.source.events[connection.eventIdx];
                input = connection.target.trigger;
                context.strokeStyle = "yellow"
            }

            context.beginPath()
            context.moveTo(output.viewPos.x, output.viewPos.y)
            context.bezierCurveTo(
                output.viewPos.x + 70, output.viewPos.y,
                input.viewPos.x - 70, input.viewPos.y,
                input.viewPos.x, input.viewPos.y
            )
            context.stroke()
        }
    }


    anchors.fill: parent
    contextType: "2d"

    onPaint: {
        context.save()
        context.clearRect(0, 0, width, height)

        context.lineWidth = 3

        drawAllConnections()

        context.restore()
    }


    MouseArea {
        id: mouseArea
        anchors.fill: parent
        enabled: true
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onPositionChanged: { appController.mousePos = Qt.point(mouseArea.mouseX, mouseArea.mouseY)}
        onPressed: {
            if (appController.selectedArg != null) {
                appController.selectedArg = null
            }
        }
    }
}