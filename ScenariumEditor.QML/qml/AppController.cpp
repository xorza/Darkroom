#include "AppController.hpp"

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

    emit nodesChanged();

}
