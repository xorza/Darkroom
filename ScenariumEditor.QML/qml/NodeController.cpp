#include "NodeController.hpp"

#include <QQuickItem>


NodeController::NodeController(QObject *parent) : QObject(parent) {
    m_trigger = new ArgumentController(this);
    m_trigger->setType(ArgumentController::ArgumentType::Trigger);
    m_trigger->setIndex(0);
    m_trigger->setName("Trigger");

    connect(m_trigger, &ArgumentController::selectedChanged, this, [this]() {
        emit selectedArgumentChanged(m_trigger);
    });
}

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

void ArgumentController::setItem(QQuickItem *item) {
    if (m_item == item) {
        return;
    }

    m_item = item;
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
    QQuickItem *const item = this->trigger()->item();
    auto pos = nodeRoot->mapFromItem(item, QPointF(0, 0));
    this->trigger()->setViewPos(pos + this->viewPos() + QPointF(item->width() / 2.0, item->height() / 2.0));
}

void NodeController::addInput(ArgumentController *const input) {
    input->setType(ArgumentController::ArgumentType::Input);
    input->setIndex(m_inputs.size());

    connect(input, &ArgumentController::selectedChanged, this, [this, input]() {
        emit selectedArgumentChanged(input);
    });

    m_inputs.push_back(input);
    emit inputsChanged();
}

void NodeController::addOutput(ArgumentController *const output) {
    output->setType(ArgumentController::ArgumentType::Output);
    output->setIndex(m_outputs.size());

    connect(output, &ArgumentController::selectedChanged, this, [this, output]() {
        emit selectedArgumentChanged(output);
    });

    m_outputs.push_back(output);
    emit outputsChanged();
}

void NodeController::addEvent(ArgumentController *const event) {
    event->setType(ArgumentController::ArgumentType::Event);
    event->setIndex(m_events.size());

    connect(event, &ArgumentController::selectedChanged, this, [this, event]() {
        emit selectedArgumentChanged(event);
    });

    m_events.push_back(event);
    emit eventsChanged();
}

void NodeController::remove() {
    emit removeRequested();
}
