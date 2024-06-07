#pragma once

#include <QtCore>

class ArgumentController : public QObject {
Q_OBJECT

    Q_PROPERTY(QString name READ name NOTIFY nameChanged)
    Q_PROPERTY(QPointF viewPos READ viewPos WRITE setViewPos NOTIFY viewPosChanged)


public:
    explicit ArgumentController(QObject *parent = nullptr) : QObject(parent) {}

    ~ArgumentController() override = default;

    [[nodiscard]] QString name() const {
        return m_name;
    }

    void setName(const QString &name);

    [[nodiscard]] QPointF viewPos() const {
        return m_viewPos;
    }

    void setViewPos(const QPointF &viewPos);

signals:

    void nameChanged();

    void viewPosChanged();

private:
    QString m_name;

    QPointF m_viewPos{};
};


class NodeController : public QObject {
Q_OBJECT

    Q_PROPERTY(QString name READ name WRITE setName NOTIFY nameChanged)
    Q_PROPERTY(QList<ArgumentController *> inputs READ inputs NOTIFY inputsChanged)
    Q_PROPERTY(QList<ArgumentController *> outputs READ outputs NOTIFY outputsChanged)
    Q_PROPERTY(QPointF viewPos READ viewPos WRITE setViewPos NOTIFY viewPosChanged)


public:
    explicit NodeController(QObject *parent = nullptr) : QObject(parent) {}

    ~NodeController() override = default;

    [[nodiscard]] QString name() const {
        return m_name;
    }

    void setName(const QString &name);

    [[nodiscard]] QList<ArgumentController *> inputs() const {
        return m_inputs;
    }

    void addInput(ArgumentController *const input) {
        m_inputs.push_back(input);
        emit inputsChanged();
    }

    [[nodiscard]] QList<ArgumentController *> outputs() const {
        return m_outputs;
    }

    void addOutput(ArgumentController *const output) {
        m_outputs.push_back(output);
        emit outputsChanged();
    }

    [[nodiscard]] QPointF viewPos() const {
        return m_viewPos;
    }

    void setViewPos(const QPointF &viewPos);

signals:

    void nameChanged();

    void inputsChanged();

    void outputsChanged();

    void viewPosChanged();


private:
    QString m_name{};
    QList<ArgumentController *> m_inputs{};
    QList<ArgumentController *> m_outputs{};
    QPointF m_viewPos{};


};
