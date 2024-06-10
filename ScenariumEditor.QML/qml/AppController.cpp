#include "AppController.hpp"


#include "NodeController.hpp"
#include "ArgumentController.hpp"
#include "ConnectionController.hpp"

#include <QQuickItem>
#include <QQuickWindow>


AppController::AppController(QObject *parent) : QObject(parent) {
    m_coreContext = std::make_unique<Ctx>();
}


void AppController::loadSample() {
    {
        auto node = new NodeController(this);
        node->setName("Node 1");

        auto input = new ArgumentController(node);
        input->setName("Input 1");
        node->addInput(input);

        input = new ArgumentController(node);
        input->setName("Input 2");
        node->addInput(input);

        auto output = new ArgumentController(node);
        output->setName("Output 2");
        node->addOutput(output);

        node->setViewPos(QPointF(100, 100));

        addNode(node);
    }

    {
        auto node = new NodeController(this);
        node->setName("Node 2");

        auto input = new ArgumentController(node);
        input->setName("Input 1");
        node->addInput(input);

        auto output = new ArgumentController(node);
        output->setName("value 2");
        node->addOutput(output);

        output = new ArgumentController(node);
        output->setName("asfahgd 2");
        node->addOutput(output);

        auto event = new ArgumentController(node);
        event->setName("event 2");
        node->addEvent(event);

        node->setViewPos(QPointF(200, 200));

        addNode(node);
    }

    {
        auto connection = new ConnectionController(this);
        connection->setSourceOutput(m_nodes[0], 0);
        connection->setTargetInput(m_nodes[1], 0);
        m_connections.append(connection);

        connection = new ConnectionController(this, ConnectionController::ConnectionType::Event);
        connection->setSourceEvent(m_nodes[1], 0);
        connection->setTargetTrigger(m_nodes[0]);
        m_connections.append(connection);
    }

    m_nodes[1]->setSelected(true);

    emit nodesChanged();
    emit connectionsChanged();
}

[[maybe_unused]] void AppController::afterSynchronizing() {
    for (auto *const node: m_nodes) {
        node->updateViewPos(mousePos());
    }
}

void AppController::addNode(NodeController *node) {
    m_nodes.append(node);

    connect(node, &NodeController::selectedChanged, this, [this, node]() {
        if (!node->selected()) {
            return;
        }

        if (this->m_selectedNode != nullptr) {
            if (this->m_selectedNode != node)
                this->m_selectedNode->setSelected(false);
        }
        this->m_selectedNode = node;
    });
    connect(node, &NodeController::selectedArgumentChanged, this, [this](ArgumentController *argument) {
        assert(argument != nullptr);

        setSelectedArg(argument);
    });
    connect(node, &NodeController::removeRequested, this, [this, node]() {
        removeNode(node);
    });

    emit nodesChanged();
}

void AppController::setSelectedArg(ArgumentController *selectedArg) {
    if (selectedArg == nullptr) {
        if (m_selectedArg != nullptr) {
            m_selectedArg->setSelected(false);
            m_selectedArg = nullptr;
            emit selectedArgChanged();
        }
        return;
    }

    if (m_selectedArg == nullptr) {
        m_selectedArg = selectedArg;
        m_selectedArg->setSelected(true);
        emit selectedArgChanged();
        return;
    }

    if (m_selectedArg == selectedArg) {
        m_selectedArg->setSelected(false);
        m_selectedArg = nullptr;
        emit selectedArgChanged();
        return;
    }

    if (!m_selectedArg->canConnectTo(selectedArg)) {
        selectedArg->setSelected(false);
        return;
    }

    auto connection = createConnection(m_selectedArg, selectedArg);

    m_selectedArg->setSelected(false);
    selectedArg->setSelected(false);
    m_selectedArg = nullptr;
    emit selectedArgChanged();
}

ConnectionController *AppController::createConnection(ArgumentController *a, ArgumentController *b) {
    assert(a->canConnectTo(b));

    ConnectionController *connection = nullptr;

    if (a->type() == ArgumentController::ArgumentType::Trigger) {
        connection = new ConnectionController(this, ConnectionController::ConnectionType::Event);
        connection->setSourceEvent(a->node(), a->index());
        connection->setTargetTrigger(b->node());
    } else if (a->type() == ArgumentController::ArgumentType::Event) {
        connection = new ConnectionController(this, ConnectionController::ConnectionType::Event);
        connection->setSourceEvent(b->node(), b->index());
        connection->setTargetTrigger(a->node());
    } else if (a->type() == ArgumentController::ArgumentType::Input) {
        connection = new ConnectionController(this, ConnectionController::ConnectionType::Data);
        connection->setTargetInput(a->node(), a->index());
        connection->setSourceOutput(b->node(), b->index());
    } else if (a->type() == ArgumentController::ArgumentType::Output) {
        connection = new ConnectionController(this, ConnectionController::ConnectionType::Data);
        connection->setTargetInput(b->node(), b->index());
        connection->setSourceOutput(a->node(), a->index());
    } else {
        assert(false);
    }

    m_connections.append(connection);
    return connection;
}

void AppController::removeNode(NodeController *node) {
    m_coreContext->remove_node(node->id());


    m_nodes.removeOne(node);

    for (auto *const connection: m_connections) {
        if (connection->source() == node || connection->target() == node) {
            m_connections.removeOne(connection);
            delete connection;
        }
    }

    if (m_selectedNode == node) {
        m_selectedNode = nullptr;
    }

    emit connectionsChanged();
    emit nodesChanged();
}

void AppController::setMousePos(const QPointF &mousePos) {
    if (m_mousePos == mousePos) {
        return;
    }

    m_mousePos = mousePos;
    emit mousePosChanged();
}
