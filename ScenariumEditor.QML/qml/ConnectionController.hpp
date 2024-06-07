#pragma once


#include <QtCore>

class NodeController;

class ConnectionController : public QObject {
Q_OBJECT

    Q_PROPERTY(NodeController *source READ source NOTIFY sourceChanged)
    Q_PROPERTY(NodeController *target READ target NOTIFY targetChanged)
    Q_PROPERTY(int inputIdx READ inputIdx NOTIFY inputIdxChanged)
    Q_PROPERTY(int outputIdx READ outputIdx NOTIFY outputIdxChanged)

public:
    explicit ConnectionController(
            NodeController *source, NodeController *target,
            int inputIdx, int outputIdx,
            QObject *parent = nullptr
    )
            : QObject(parent), m_source(source), m_target(target), m_inputIdx(inputIdx), m_outputIdx(outputIdx) {}

    ~ConnectionController() override = default;

    [[nodiscard]] NodeController *source() const {
        return m_source;
    }

    [[nodiscard]] NodeController *target() const {
        return m_target;
    }

    [[nodiscard]] int inputIdx() const {
        return m_inputIdx;
    }

    [[nodiscard]] int outputIdx() const {
        return m_outputIdx;
    }

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
