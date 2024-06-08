#include "AppController.hpp"


#include "NodeController.hpp"
#include "ConnectionController.hpp"

#include <QQuickItem>
#include <QQuickWindow>

void AppController::loadSample() {
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

    m_nodes.append(node);


    node = new NodeController(this);
    node->setName("Node 2");

    input = new ArgumentController(node);
    input->setName("Input 1");
    node->addInput(input);

    output = new ArgumentController(node);
    output->setName("value 2");
    node->addOutput(output);

    output = new ArgumentController(node);
    output->setName("asfahgd 2");
    node->addOutput(output);

    m_nodes.append(node);

    auto connection = new ConnectionController(this);
    connection->setSource(m_nodes[0], 0);
    connection->setTarget(m_nodes[1], 0);
    m_connections.append(connection);


    emit nodesChanged();
    emit connectionsChanged();
}

[[maybe_unused]] void AppController::afterSynchronizing() {
    for (auto *const node: m_nodes) {
        node->updateViewPos();
    }
}
