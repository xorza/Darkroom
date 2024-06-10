#pragma once

#include "../src/utils/uuid.hpp"

#include <QtCore>
#include <QQuickItem>

class NodeController;

class ArgumentController : public QObject {
Q_OBJECT

    Q_PROPERTY(QString name READ name NOTIFY nameChanged)
    Q_PROPERTY(QPointF viewPos READ viewPos WRITE setViewPos NOTIFY viewPosChanged)
    Q_PROPERTY(QQuickItem *item READ item WRITE setItem)

    Q_PROPERTY(bool selected READ selected WRITE setSelected NOTIFY selectedChanged)
    Q_PROPERTY(ArgumentType type READ type)


public:
    enum class ArgumentType {
        Input,
        Output,
        Event,
        Trigger
    };

    Q_ENUM(ArgumentType)

    explicit ArgumentController(NodeController *parent);

    ~ArgumentController() override = default;

    [[nodiscard]] QString name() const {
        return m_name;
    }

    void setName(const QString &name);

    [[nodiscard]] QPointF viewPos() const {
        return m_viewPos;
    }

    void setViewPos(const QPointF &viewPos);

    [[nodiscard]] QQuickItem *item() const {
        return m_item;
    }

    void setItem(QQuickItem *item);

    [[nodiscard]] ArgumentType type() const {
        return m_type;
    }

    void setType(ArgumentType type) {
        m_type = type;
    }

    [[nodiscard]] uint32_t index() const {
        return m_idx;
    }

    void setIndex(uint32_t index) {
        m_idx = index;
    }

    [[nodiscard]] NodeController *node() const {
        return m_parent;
    }

    [[nodiscard]] bool selected() const {
        return m_selected;
    }

    void setSelected(bool selected);

    bool canConnectTo(ArgumentController *other) const;

signals:

    void nameChanged();

    void viewPosChanged();

    void selectedChanged();

public slots:


private:
    QString m_name{};
    QPointF m_viewPos{};
    QQuickItem *m_item{};
    ArgumentType m_type{};
    uint32_t m_idx{};
    NodeController *m_parent{};
    bool m_selected = false;
};


class NodeController : public QObject {
Q_OBJECT

    Q_PROPERTY(QString name READ name WRITE setName NOTIFY nameChanged)
    Q_PROPERTY(QList<ArgumentController *> inputs READ inputs NOTIFY inputsChanged)
    Q_PROPERTY(QList<ArgumentController *> outputs READ outputs NOTIFY outputsChanged)
    Q_PROPERTY(QList<ArgumentController *> events READ events NOTIFY eventsChanged)
    Q_PROPERTY(ArgumentController *trigger READ trigger NOTIFY triggerChanged)

    Q_PROPERTY(QPointF viewPos READ viewPos WRITE setViewPos NOTIFY viewPosChanged)
    Q_PROPERTY(QQuickItem *item READ item WRITE setItem NOTIFY itemChanged)

    Q_PROPERTY(bool selected READ selected WRITE setSelected NOTIFY selectedChanged)


public:
    explicit NodeController(QObject *parent = nullptr);

    ~NodeController() override = default;

    [[nodiscard]] QString name() const {
        return m_name;
    }

    void setName(const QString &name);

    [[nodiscard]] const QList<ArgumentController *> &inputs() const {
        return m_inputs;
    }

    void addInput(ArgumentController *input);

    [[nodiscard]] const QList<ArgumentController *> &outputs() const {
        return m_outputs;
    }

    void addOutput(ArgumentController *output);

    [[nodiscard]] QPointF viewPos() const {
        return m_viewPos;
    }

    void setViewPos(const QPointF &viewPos);

    [[nodiscard]] QQuickItem *item() const {
        return m_item;
    }

    void setItem(QQuickItem *item);


    [[nodiscard]] const QList<ArgumentController *> &events() const {
        return m_events;
    }

    void addEvent(ArgumentController *event);


    [[nodiscard]] bool selected() const {
        return m_selected;
    }

    void setSelected(bool selected);

    [[nodiscard]] ArgumentController *trigger() const {
        return m_trigger;
    }

    [[nodiscard]] uuid id() const {
        return m_id;
    }

    void updateViewPos();

signals:

    void nameChanged();

    void inputsChanged();

    void outputsChanged();

    void viewPosChanged();

    void itemChanged();

    void eventsChanged();

    void selectedChanged();

    void triggerChanged();

    void selectedArgumentChanged(ArgumentController *arg);

    void removeRequested();

public slots:

    void remove();

private:
    QString m_name{};
    QList<ArgumentController *> m_inputs{};
    QList<ArgumentController *> m_outputs{};
    QPointF m_viewPos{};
    QQuickItem *m_item{};
    QList<ArgumentController *> m_events{};
    bool m_selected = false;
    ArgumentController *m_trigger{};
    uuid m_id{};
};
