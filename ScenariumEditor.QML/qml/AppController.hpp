#pragma once

#include "ConnectionController.hpp"
#include "NodeController.hpp"
#include "../src/CoreContext.hpp"

#include <QtCore>
#include <QQmlComponent>

#include <memory>


class AppController : public QObject {
Q_OBJECT

    Q_PROPERTY(QList<NodeController *> nodes READ nodes NOTIFY nodesChanged)
    Q_PROPERTY(QList<ConnectionController *> connections READ connections NOTIFY connectionsChanged)

    Q_PROPERTY(ArgumentController *selectedArg READ selectedArg WRITE setSelectedArg NOTIFY selectedArgChanged)

    Q_PROPERTY(QPointF mousePos READ mousePos WRITE setMousePos NOTIFY mousePosChanged)

public:
    explicit AppController(QObject *parent);

    ~AppController() override = default;

    [[nodiscard]] const QList<NodeController *> &nodes() const {
        return m_nodes;
    }

    [[nodiscard]] const QList<ConnectionController *> &connections() const {
        return m_connections;
    }

    void addNode(NodeController *node);

    void removeNode(NodeController *node);

    [[nodiscard]] NodeController *selectedNode() const {
        return m_selectedNode;
    }

    [[nodiscard]] ArgumentController *selectedArg() const {
        return m_selectedArg;
    }

    void setSelectedArg(ArgumentController *selectedArg);

    [[nodiscard]] QPointF mousePos() const {
        return m_mousePos;
    }

    void setMousePos(const QPointF &mousePos);


    void loadSample();

signals:

    void nodesChanged();

    void connectionsChanged();

    void selectedArgChanged();

    void mousePosChanged();

public slots:

    void afterSynchronizing();

private:
    QList<NodeController *> m_nodes{};
    QList<ConnectionController *> m_connections{};
    NodeController *m_selectedNode{};
    ArgumentController *m_selectedArg{};
    QPointF m_mousePos{};
    std::unique_ptr<Ctx> m_coreContext;


    ConnectionController *createConnection(ArgumentController *a, ArgumentController *b);
};
