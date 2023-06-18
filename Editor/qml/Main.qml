import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import "."

Rectangle {
    color: Constants.backgroundColor

    ColumnLayout {
        width: parent.width

        ListView {
            id: functionListView
            width: 250
            height: contentHeight

            model: app_model.functions

            delegate: Item {
                width: functionNode.width + Constants.defaultMargin
                height: functionNode.height + Constants.defaultMargin

                Node {
                    id: functionNode
                    anchors.centerIn: parent
                    func: modelData
                }
            }
        }
    }
}

