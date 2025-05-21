#pragma once

#include "NodeController.hpp"

#include <QtCore>


class ConnectionController : public QObject {
Q_OBJECT

    Q_PROPERTY(NodeController *source READ source NOTIFY sourceChanged)
    Q_PROPERTY(int outputIdx READ outputIdx NOTIFY outputIdxChanged)
    Q_PROPERTY(NodeController *target READ target NOTIFY targetChanged)
    Q_PROPERTY(int inputIdx READ inputIdx NOTIFY inputIdxChanged)
    Q_PROPERTY(int eventIdx READ eventIdx NOTIFY eventIdxChanged)
    Q_PROPERTY(ConnectionType connectionType READ connectionType NOTIFY connectionTypeChanged)

public:
    enum class ConnectionType {
        Data,
        Event
    };

    Q_ENUM(ConnectionType)

    explicit ConnectionController(QObject *parent)
            : QObject(parent) {}

    ConnectionController(QObject *parent, ConnectionType connectionType)
            : QObject(parent), m_connectionType(connectionType) {}

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

    [[nodiscard]] int eventIdx() const {
        return m_eventIdx;
    }


    void setSourceOutput(NodeController *source, uint32_t outputIdx);

    void setTargetInput(NodeController *target, uint32_t inputIdx);

    void setSourceEvent(NodeController *source, uint32_t eventIdx);

    void setTargetTrigger(NodeController *target);

    [[nodiscard]] ConnectionType connectionType() const {
        return m_connectionType;
    }

signals:

    void sourceChanged();

    void targetChanged();

    void inputIdxChanged();

    void outputIdxChanged();

    void eventIdxChanged();

    void connectionTypeChanged();

private:
    NodeController *m_source = nullptr;
    NodeController *m_target = nullptr;
    uint32_t m_inputIdx = 0;
    uint32_t m_outputIdx = 0;
    uint32_t m_eventIdx = 0;
    ConnectionType m_connectionType = ConnectionType::Data;
};
