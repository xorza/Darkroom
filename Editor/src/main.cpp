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

    QScopedPointer<AppModel> app_model(new AppModel);
    QQmlApplicationEngine engine;

    engine.rootContext()->setContextProperty("app_model", app_model.data());

    engine.load("qrc:/qml/Window.qml");
    if (engine.rootObjects().isEmpty()) {
        return -1;
    }

    int32_t result = QGuiApplication::exec();

    return result;
}
