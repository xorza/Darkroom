import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import com.cssodessa.NodeController
import com.cssodessa.ArgumentController
import com.cssodessa.NodeLibrary
import "Enums.js" as Enums

ColumnLayout {
    property FuncLibrary funcLibrary

    Repeater {
        model: funcLibrary.funcs
        delegate: Item {
            width: parent.width
            height: 50
            RowLayout {
                width: parent.width
                height: 50
                spacing: 10
                Text {
                    text: model.name
                    font.pixelSize: 20
                }
                Button {
                    text: "Add"
                    onClicked: {
                        var node = NodeController.createNode(model)
                        node.x = 100
                        node.y = 100
                        NodeController.addNode(node)
                    }
                }
            }
        }
    }
}