import QtQuick
import com.cssodessa.AppController
import com.cssodessa.NodeController
import com.cssodessa.ConnectionController
import com.cssodessa.ArgumentController


Canvas {
    id: canvas

    property AppController appController
    property point mousePos

    function repaintConnections() {
        canvas.requestPaint()
    }

    anchors.fill: parent
    anchors.margins: 10
    contextType: "2d"

    onPaint: {
        context.save()
        context.clearRect(0, 0, width, height)

        context.lineWidth = 3

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

        if (appController.selectedArg != null) {
            if (appController.selectedArg.type === ArgumentController.ArgumentType.Input
                || appController.selectedArg.type === ArgumentController.ArgumentType.Output) {
                context.strokeStyle = "red"
            } else {
                context.strokeStyle = "yellow"
            }

            context.beginPath()

            if (appController.selectedArg.type === ArgumentController.ArgumentType.Input
                || appController.selectedArg.type === ArgumentController.ArgumentType.Trigger) {
                context.moveTo(canvas.mousePos.x, canvas.mousePos.y)
                context.bezierCurveTo(
                    canvas.mousePos.x + 70, canvas.mousePos.y,
                    appController.selectedArg.viewPos.x - 70, appController.selectedArg.viewPos.y,
                    appController.selectedArg.viewPos.x, appController.selectedArg.viewPos.y
                )
            } else {
                context.moveTo(canvas.mousePos.x, canvas.mousePos.y)
                context.bezierCurveTo(
                    canvas.mousePos.x - 70, canvas.mousePos.y,
                    appController.selectedArg.viewPos.x + 70, appController.selectedArg.viewPos.y,
                    appController.selectedArg.viewPos.x, appController.selectedArg.viewPos.y
                )
            }

            context.stroke()
        }

        context.restore()
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onPositionChanged: {
            canvas.mousePos = Qt.point(mouse.x, mouse.y)
        }
        onPressed: {
            if (appController.selectedArg != null) {
                appController.selectedArg = null
            }
        }
    }
}