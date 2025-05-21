#include "ArgumentController.hpp"

#include "NodeController.hpp"


ArgumentController::ArgumentController(NodeController *parent) : QObject(parent) {
    m_parent = parent;
}


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

void ArgumentController::setPin(QQuickItem *item) {
    if (m_pin == item) {
        return;
    }

    m_pin = item;
}

void ArgumentController::setSelected(bool selected) {
    if (m_selected == selected) {
        return;
    }

    m_selected = selected;
    emit selectedChanged();

}

bool ArgumentController::canConnectTo(ArgumentController *other) const {
    if (m_type == ArgumentType::Trigger) {
        return other->type() == ArgumentType::Event;
    }

    if (m_type == ArgumentType::Event) {
        return other->type() == ArgumentType::Trigger;
    }

    if (m_type == ArgumentType::Input) {
        return other->type() == ArgumentType::Output;
    }

    if (m_type == ArgumentType::Output) {
        return other->type() == ArgumentType::Input;
    }

    assert(false);
    return false;
}

void ArgumentController::setMouseArea(QQuickItem *item) {
    if (m_mouseArea == item) {
        return;
    }

    m_mouseArea = item;
}
