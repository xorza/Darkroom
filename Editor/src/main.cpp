#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QtQuick>

#include "Counter.h"

int main(int argc, char *argv[]) {
    QGuiApplication app(argc, argv);

    Counter counter;

    QQmlApplicationEngine engine;
    engine.rootContext()->setContextProperty("counter", &counter);
    engine.load("qrc:/qml/Window.qml");

    return QGuiApplication::exec();
}
