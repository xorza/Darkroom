#include "ConnectionController.hpp"

#include <cassert>


void ConnectionController::setSourceOutput(NodeController *source, int outputIdx) {
    if (m_source == source && m_outputIdx == outputIdx) {
        return;
    }

    assert(m_connectionType == ConnectionType::Data);

    m_source = source;
    m_outputIdx = outputIdx;

    emit sourceChanged();
    emit outputIdxChanged();

}

void ConnectionController::setTargetInput(NodeController *target, int inputIdx) {
    if (m_target == target && m_inputIdx == inputIdx) {
        return;
    }

    assert(m_connectionType == ConnectionType::Data);

    m_target = target;
    m_inputIdx = inputIdx;

    emit targetChanged();
    emit inputIdxChanged();

}

void ConnectionController::setSourceEvent(NodeController *source, int eventIdx) {
    if (m_source == source && m_eventIdx == eventIdx) {
        return;
    }

    assert(m_connectionType == ConnectionType::Event);

    m_source = source;
    m_eventIdx = eventIdx;

    emit sourceChanged();
    emit eventIdxChanged();

}

void ConnectionController::setTargetTrigger(NodeController *target) {
    if (m_target == target) {
        return;
    }

    assert(m_connectionType == ConnectionType::Event);

    m_target = target;

    emit targetChanged();

}

