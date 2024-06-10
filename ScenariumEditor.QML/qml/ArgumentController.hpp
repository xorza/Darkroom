#pragma once


#include "../src/utils/uuid.hpp"

#include <QtCore>
#include <QQuickItem>


class NodeController;

class ArgumentController : public QObject {
Q_OBJECT

    Q_PROPERTY(QString name READ name NOTIFY nameChanged)
    Q_PROPERTY(QPointF viewPos READ viewPos WRITE setViewPos NOTIFY viewPosChanged)
    Q_PROPERTY(QQuickItem *pin READ pin WRITE setPin)
    Q_PROPERTY(QQuickItem *mouseArea READ mouseArea WRITE setMouseArea)

    Q_PROPERTY(bool selected READ selected WRITE setSelected NOTIFY selectedChanged)
    Q_PROPERTY(bool highlighted READ highlighted WRITE setHighlighted NOTIFY highlightedChanged)

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

    [[nodiscard]] QQuickItem *pin() const {
        return m_pin;
    }

    void setPin(QQuickItem *item);

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

    [[nodiscard]] QQuickItem *mouseArea() const {
        return m_mouseArea;
    }

    void setMouseArea(QQuickItem *item);

    void setSelected(bool selected);

    bool canConnectTo(ArgumentController *other) const;

        [[nodiscard]] bool highlighted() const {
            return m_highlighted;
        }

        void setHighlighted(bool highlighted) ;

signals:

    void nameChanged();

    void viewPosChanged();

    void selectedChanged();

    void highlightedChanged();

public slots:


private:
    QString m_name{};
    QPointF m_viewPos{};
    QQuickItem *m_pin{};
    ArgumentType m_type{};
    uint32_t m_idx{};
    NodeController *m_parent{};
    bool m_selected = false;
    QQuickItem *m_mouseArea{};
    bool m_highlighted = false;
};

