#include "AppController.hpp"

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

    emit nodesChanged();

}


void AppController::onRendered(QQuickWindow *window) {
    for (auto *const node: m_nodes) {
        QQuickItem *const nodeRoot = qobject_cast<QQuickItem *>(node->item());

        for (auto *const input: node->inputs()) {
            QQuickItem *const item = qobject_cast<QQuickItem *>(input->item());
            auto pos = nodeRoot->mapFromItem(item, QPointF(0, 0));
            input->setViewPos(pos);
        }
        for (auto *const output: node->outputs()) {
            QQuickItem *const item = qobject_cast<QQuickItem *>(output->item());
            auto pos = nodeRoot->mapFromItem(item, QPointF(0, 0));
            output->setViewPos(pos);
        }
    }

}
