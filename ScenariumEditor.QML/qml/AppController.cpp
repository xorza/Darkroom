#include "AppController.hpp"


#include "NodeController.hpp"
#include "ConnectionController.hpp"

#include <QQuickItem>
#include <QQuickWindow>

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

        addNode(node);
    }

    {
        auto connection = new ConnectionController(this);
        connection->setSource(m_nodes[0], 0);
        connection->setTarget(m_nodes[1], 0);
        m_connections.append(connection);
    }

    m_nodes[1]->setSelected(true);

    emit nodesChanged();
    emit connectionsChanged();
}

[[maybe_unused]] void AppController::afterSynchronizing() {
    for (auto *const node: m_nodes) {
        node->updateViewPos();
    }
}

void AppController::addNode(NodeController *node)  {
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

    emit nodesChanged();
}
