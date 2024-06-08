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

void ArgumentController::setItem(QQuickItem *item) {
    if (m_item == item) {
        return;
    }

    m_item = item;
    emit itemChanged();
}

void NodeController::setSelected(bool selected) {
    if (m_selected == selected) {
        return;
    }

    m_selected = selected;
    emit selectedChanged();
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

void NodeController::setItem(QQuickItem *item) {
    if (m_item == item) {
        return;
    }

    m_item = item;
    emit itemChanged();
}

void NodeController::updateViewPos() {
    QQuickItem *const nodeRoot = qobject_cast<QQuickItem *>(this->item());

    for (auto *const input: this->inputs()) {
        auto item = input->item();
        auto pos = nodeRoot->mapFromItem(item, QPointF(0, 0));
        input->setViewPos(pos + this->viewPos() + QPointF(item->width() / 2.0, item->height() / 2.0));
    }
    for (auto *const output: this->outputs()) {
        auto item = output->item();
        auto pos = nodeRoot->mapFromItem(item, QPointF(0, 0));
        output->setViewPos(pos + this->viewPos() + QPointF(item->width() / 2.0, item->height() / 2.0));
    }
    for (auto *const event: this->events()) {
        auto item = event->item();
        auto pos = nodeRoot->mapFromItem(item, QPointF(0, 0));
        event->setViewPos(pos + this->viewPos() + QPointF(item->width() / 2.0, item->height() / 2.0));
    }
    QQuickItem *const item = this->triggerItem();
    auto pos = nodeRoot->mapFromItem(item, QPointF(0, 0));
    this->setTriggerViewPos(pos + this->viewPos() + QPointF(item->width() / 2.0, item->height() / 2.0));
}

void NodeController::setTriggerViewPos(const QPointF &triggerViewPos) {
    if (m_triggerViewPos == triggerViewPos) {
        return;
    }

    m_triggerViewPos = triggerViewPos;
    emit triggerViewPosChanged();
}

void NodeController::setTriggerItem(QQuickItem *triggerItem) {
    if (m_triggerItem == triggerItem) {
        return;
    }

    m_triggerItem = triggerItem;
    emit triggerItemChanged();
}

