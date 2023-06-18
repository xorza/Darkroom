#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QtQuick>

#include "app_model.hpp"
#include "function_info.hpp"

int main(int argc, char *argv[]) {
    QGuiApplication app(argc, argv);

    qmlRegisterUncreatableType<QmlFunctionInfo>("com.csso", 1, 0, "QmlFunctionInfo", "");
    qmlRegisterUncreatableType<QmlArgInfo>("com.csso", 1, 0, "QmlArgInfo", "");

    int32_t result = 0;
    AppModel app_model{};
    {
        QQmlApplicationEngine engine;

        engine.rootContext()->setContextProperty("app_model", &app_model);

        engine.load("qrc:/qml/Window.qml");
        if (engine.rootObjects().isEmpty()) {
            return -1;
        }

        result = QGuiApplication::exec();
    }
    return result;
}
