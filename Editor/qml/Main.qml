import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    color: Constants.backgroundColor

    RowLayout {
        Button {
            id: button
            text: "Increment"
            onClicked: counter.increment()
        }

        Label {
            text: "Count: " + counter.count
            Layout.alignment: Qt.AlignVCenter
        }
    }

}

