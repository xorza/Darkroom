#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QtQuick>

#include "Counter.h"

int main(int argc, char *argv[]) {
    QGuiApplication app(argc, argv);

    Counter counter;


//    QQmlApplicationEngine engine;
//    engine.rootContext()->setContextProperty("counter", &counter);

    const QUrl url(QStringLiteral("qrc:/qml/main.qml"));
//    QObject::connect(&engine,
//                     &QQmlApplicationEngine::objectCreated,
//                     &app,
//                     [url](QObject *obj, const QUrl &objUrl) {
//                         if (!obj && url == objUrl)
//                             QCoreApplication::exit(-1);
//                     },
//                     Qt::QueuedConnection);
//
//    engine.load(url);

    QQuickView view(url);

    view.show();

    return QGuiApplication::exec();
}
