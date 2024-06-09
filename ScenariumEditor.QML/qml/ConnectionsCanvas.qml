import QtQuick
import com.cssodessa.AppController
import com.cssodessa.NodeController
import com.cssodessa.ConnectionController


Canvas {
    id: canvas

    property AppController appController

    function repaintConnections() {
        canvas.requestPaint()
    }

    anchors.fill: parent
    anchors.margins: 10
    contextType: "2d"

    onPaint: {
        context.save()
        context.clearRect(0, 0, width, height)

        context.strokeStyle = "orange"
        context.lineWidth = 3

        for (let i = 0; i < appController.connections.length; i++) {
            const connection = appController.connections[i];
            let input;
            let output;
            if (connection.connectionType === ConnectionController.ConnectionType.Data) {
                output = connection.source.outputs[connection.outputIdx];
                input = connection.target.inputs[connection.inputIdx];
            } else {
                output = connection.source.events[connection.eventIdx];
                input = connection.target.trigger;
            }

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