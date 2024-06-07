#pragma once

#include <QtCore>

class InputController : public QObject {
Q_OBJECT

    Q_PROPERTY(QString name READ name NOTIFY nameChanged)

public:
    explicit InputController(QObject *parent = nullptr) : QObject(parent) {}

    ~InputController() override = default;

    [[nodiscard]] QString name() const {
        return m_name;
    }

    void setName(const QString &name);

signals:

    void nameChanged();

private:
    QString m_name;

};


class NodeController : public QObject {
Q_OBJECT

    Q_PROPERTY(QString name READ name WRITE setName NOTIFY nameChanged)
    Q_PROPERTY(QList<InputController *> inputs READ inputs NOTIFY inputsChanged)

public:
    explicit NodeController(QObject *parent = nullptr) : QObject(parent) {}

    ~NodeController() override = default;

    [[nodiscard]] QString name() const {
        return m_name;
    }

    void setName(const QString &name);

    [[nodiscard]] QList<InputController *> inputs() const {
        return m_inputs;
    }

    void addInput(InputController *const input) {
        m_inputs.push_back(input);
        emit inputsChanged();
    }

signals:

    void nameChanged();

    void inputsChanged();

private:
    QString m_name{};
    QList<InputController *> m_inputs{};
};
