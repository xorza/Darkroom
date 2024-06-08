#include "ConnectionController.hpp"


void ConnectionController::setSource(NodeController *source, int outputIdx) {
    if (m_source == source && m_outputIdx == outputIdx) {
        return;
    }

    m_source = source;
    m_outputIdx = outputIdx;

    emit sourceChanged();
    emit outputIdxChanged();

}

void ConnectionController::setTarget(NodeController *target, int inputIdx) {
    if (m_target == target && m_inputIdx == inputIdx) {
        return;
    }

    m_target = target;
    m_inputIdx = inputIdx;

    emit targetChanged();
    emit inputIdxChanged();

}