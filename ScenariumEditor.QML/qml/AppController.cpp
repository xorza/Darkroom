#include "AppController.hpp"

void AppController::loadSample() {
    auto node = new NodeController(this);
    node->setName("Node 1");

    auto input = new InputController(node);
    input->setName("Input 1");
    node->addInput(input);

    input = new InputController(node);
    input->setName("Input 2");
    node->addInput(input);

    m_nodes.append(node);

    node = new NodeController(this);
    node->setName("Node 2");

    input = new InputController(node);
    input->setName("Input 1");
    node->addInput(input);

    m_nodes.append(node);

    emit nodesChanged();

}
