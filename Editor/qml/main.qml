import QtQuick 2.12
import QtQuick.Controls 2.12

ApplicationWindow {
    visible: true
    width: 640
    height: 480
    title: qsTr("Nodeshop")

    Button {
        text: "Increment"
        onClicked: counter.increment()
    }

    Label {
        text: "Count: " + counter.count
    }
}
