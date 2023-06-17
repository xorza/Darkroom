#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QtQuick>

#include "graph.hpp"

void init() {
    graph_c_api::init();

    auto funcs = graph_c_api::get_functions_info();

}

void deinit() {
    graph_c_api::deinit();
}

int main(int argc, char *argv[]) {
    init();

    QGuiApplication app(argc, argv);
    QQmlApplicationEngine engine;
    engine.load("qrc:/qml/Window.qml");
//    engine.rootContext()->setContextProperty("counter", &counter);
    int result = QGuiApplication::exec();

    deinit();

    return result;
}
