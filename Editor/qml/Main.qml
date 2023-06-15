import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {

    RowLayout {
        Button {
            text: "Increment"
            onClicked: counter.increment()
        }

        Label {
            text: "Count: " + counter.count
            Layout.alignment: Qt.AlignVCenter
        }
    }

}