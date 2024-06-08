#include "NodeController.hpp"

#include <QQuickItem>


void ArgumentController::setName(const QString &name) {
    if (m_name == name) {
        return;
    }

    m_name = name;
    emit nameChanged();

}

void ArgumentController::setViewPos(const QPointF &viewPos) {
    if (m_viewPos == viewPos) {
        return;
    }

    m_viewPos = viewPos;
    emit viewPosChanged();
}

void ArgumentController::setItem(QObject *item) {
    if (m_item == item) {
        return;
    }

    m_item = item;
    emit itemChanged();
}

void NodeController::setName(const QString &name) {
    if (m_name == name) {
        return;
    }

    m_name = name;
    emit nameChanged();
}

void NodeController::setViewPos(const QPointF &viewPos) {
    if (m_viewPos == viewPos) {
        return;
    }

    m_viewPos = viewPos;
    emit viewPosChanged();
}

void NodeController::setItem(QObject *item) {
    if (m_item == item) {
        return;
    }

    m_item = item;
    emit itemChanged();
}

void NodeController::updateViewPos() {
    QQuickItem *const nodeRoot = qobject_cast<QQuickItem *>(this->item());

    for (auto *const input: this->inputs()) {
        QQuickItem *const item = qobject_cast<QQuickItem *>(input->item());
        auto pos = nodeRoot->mapFromItem(item, QPointF(0, 0));
        input->setViewPos(pos + this->viewPos() + QPointF(item->width() / 2.0, item->height() / 2.0));
    }
    for (auto *const output: this->outputs()) {
        QQuickItem *const item = qobject_cast<QQuickItem *>(output->item());
        auto pos = nodeRoot->mapFromItem(item, QPointF(0, 0));
        output->setViewPos(pos + this->viewPos() + QPointF(item->width() / 2.0, item->height() / 2.0));
    }
}
