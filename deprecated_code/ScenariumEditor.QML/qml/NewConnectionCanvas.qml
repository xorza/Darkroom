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


    function drawNewConnection() {
        if (appController.selectedArg == null) {
            return
        }
        if (appController.selectedArg.type === ArgumentController.ArgumentType.Input
            || appController.selectedArg.type === ArgumentController.ArgumentType.Output) {
            context.strokeStyle = "red"
        } else {
            context.strokeStyle = "yellow"
        }

        context.beginPath()

        if (appController.selectedArg.type === ArgumentController.ArgumentType.Input
            || appController.selectedArg.type === ArgumentController.ArgumentType.Trigger) {
            context.moveTo(appController.mousePos.x, appController.mousePos.y)
            context.bezierCurveTo(
                appController.mousePos.x + 70, appController.mousePos.y,
                appController.selectedArg.viewPos.x - 70, appController.selectedArg.viewPos.y,
                appController.selectedArg.viewPos.x, appController.selectedArg.viewPos.y
            )
        } else {
            context.moveTo(appController.mousePos.x, appController.mousePos.y)
            context.bezierCurveTo(
                appController.mousePos.x - 70, appController.mousePos.y,
                appController.selectedArg.viewPos.x + 70, appController.selectedArg.viewPos.y,
                appController.selectedArg.viewPos.x, appController.selectedArg.viewPos.y
            )
        }

        context.stroke()
    }

    anchors.fill: parent
    contextType: "2d"

    onPaint: {
        context.save()
        context.clearRect(0, 0, width, height)

        context.lineWidth = 3

        drawNewConnection()

        context.restore()
    }

    MouseArea {
        id: mouseArea
        enabled: appController.selectedArg != null
        anchors.fill: parent
        hoverEnabled: true
        propagateComposedEvents: true

        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onPositionChanged: {
            appController.mousePos = Qt.point(mouseArea.mouseX, mouseArea.mouseY)
            mouse.accepted = false
        }
        onPressed: {
            mouse.accepted = false
        }
    }
}