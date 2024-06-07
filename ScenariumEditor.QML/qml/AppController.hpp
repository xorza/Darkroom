#pragma once

#include "NodeController.hpp"

#include <QtCore>

class QQuickWindow;

class AppController : public QObject {
Q_OBJECT

    Q_PROPERTY(QList<NodeController *> nodes READ nodes NOTIFY nodesChanged)

public:
    explicit AppController(QObject *parent = nullptr) : QObject(parent) {}

    ~AppController() override = default;

    [[nodiscard]] const QList<NodeController *>& nodes() const {
        return m_nodes;
    }

    void loadSample();

signals:

    void nodesChanged();

public slots:

    void onRendered(QQuickWindow *window);

private:
    QList<NodeController *> m_nodes{};

};
