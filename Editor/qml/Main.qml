import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import "."

Rectangle {
    color: Constants.backgroundColor

    Row {
        Button {
            id: button
            text: "Increment"
            onClicked: counter.increment()
        }

        Label {
            text: "Count: " + counter.count
            anchors.verticalCenter: parent.verticalCenter
        }
    }

}

