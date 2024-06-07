#pragma once

#include "NodeController.hpp"

#include <QtCore>

class AppController : public QObject {
Q_OBJECT

    Q_PROPERTY(QList<NodeController *> nodes READ nodes NOTIFY nodesChanged)

public:
    explicit AppController(QObject *parent = nullptr) : QObject(parent) {}

    ~AppController() override = default;

    [[nodiscard]] QList<NodeController *> nodes() const {
        return m_nodes;
    }

    void loadSample();

signals:

    void nodesChanged();

private:
    QList<NodeController *> m_nodes{};

};
