#pragma once

#include "NodeController.hpp"

#include <QtCore>


class ConnectionController : public QObject {
Q_OBJECT

    Q_PROPERTY(NodeController *source READ source NOTIFY sourceChanged)
    Q_PROPERTY(int outputIdx READ outputIdx NOTIFY outputIdxChanged)
    Q_PROPERTY(NodeController *target READ target NOTIFY targetChanged)
    Q_PROPERTY(int inputIdx READ inputIdx NOTIFY inputIdxChanged)

public:
    explicit ConnectionController(QObject *parent = nullptr)
            : QObject(parent) {}

    ~ConnectionController() override = default;

    [[nodiscard]] NodeController *source() const {
        return m_source;
    }

    [[nodiscard]] int outputIdx() const {
        return m_outputIdx;
    }

    [[nodiscard]] NodeController *target() const {
        return m_target;
    }

    [[nodiscard]] int inputIdx() const {
        return m_inputIdx;
    }

    void setSource(NodeController *source, int outputIdx);

    void setTarget(NodeController *target, int inputIdx);

signals:

    void sourceChanged();

    void targetChanged();

    void inputIdxChanged();

    void outputIdxChanged();

private:
    NodeController *m_source = nullptr;
    NodeController *m_target = nullptr;
    int m_inputIdx = 0;
    int m_outputIdx = 0;
};
