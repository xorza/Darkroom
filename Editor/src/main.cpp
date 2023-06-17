#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QtQuick>

#include "app_model.hpp"

int main(int argc, char *argv[]) {
    QGuiApplication app(argc, argv);
    QQmlApplicationEngine engine;
    engine.load("qrc:/qml/Window.qml");

    AppModel app_model{};
    engine.rootContext()->setContextProperty("counter", &app_model);

    int result = QGuiApplication::exec();
    return result;
}
