#pragma once

#include "ConnectionController.hpp"
#include "NodeController.hpp"

#include <QtCore>


class AppController : public QObject {
Q_OBJECT

    Q_PROPERTY(QList<NodeController *> nodes READ nodes NOTIFY nodesChanged)
    Q_PROPERTY(QList<ConnectionController *> connections READ connections NOTIFY connectionsChanged)

    Q_PROPERTY(ArgumentController *selectedArg READ selectedArg WRITE setSelectedArg NOTIFY selectedArgChanged)

public:
    explicit AppController(QObject *parent = nullptr) : QObject(parent) {}

    ~AppController() override = default;

    [[nodiscard]] const QList<NodeController *> &nodes() const {
        return m_nodes;
    }

    [[nodiscard]] const QList<ConnectionController *> &connections() const {
        return m_connections;
    }

    void addNode(NodeController *node);

    [[nodiscard]] ArgumentController *selectedArg() const {
        return m_selectedArg;
    }

    void setSelectedArg(ArgumentController *selectedArg);



    void loadSample();

signals:

    void nodesChanged();

    void connectionsChanged();

    void selectedArgChanged();

public slots:

    void afterSynchronizing();

private:
    QList<NodeController *> m_nodes{};
    QList<ConnectionController *> m_connections{};
    NodeController *m_selectedNode{};
    ArgumentController *m_selectedArg{};

    ConnectionController* createConnection(ArgumentController* a, ArgumentController* b);
};
