#pragma once

#include <QObject>
#include <QString>
#include <QUuid>
#include <QVariant>
#include <utility>


namespace graph_c_api {
    struct FunctionInfo;
}


class QmlArgInfo : public QObject {
Q_OBJECT

    Q_PROPERTY(QString name READ name CONSTANT)
    Q_PROPERTY(int type READ type CONSTANT)

public:
    explicit QmlArgInfo(
            QString name,
            int type,
            QObject *parent
    ) : QObject(parent), m_name(std::move(name)), m_type(type) {}

    [[nodiscard]] const QString &name() const { return m_name; }
    int type() const { return m_type; }

private:
    QString m_name;
    int m_type;
};

class QmlFunctionInfo : public QObject {
Q_OBJECT

    Q_PROPERTY(QString name READ name CONSTANT)
    Q_PROPERTY(QUuid uuid READ uuid CONSTANT)
    Q_PROPERTY(QList<QmlArgInfo *> inputs READ inputs CONSTANT)
    Q_PROPERTY(QList<QmlArgInfo *> outputs READ outputs CONSTANT)

public:
    explicit QmlFunctionInfo(
            QString name,
            const QUuid &uuid,
            QObject *parent
    ) : QObject(parent), m_name(std::move(name)), m_uuid(uuid) {}

    explicit QmlFunctionInfo(
            const graph_c_api::FunctionInfo &func,
            QObject *parent
    );

    [[nodiscard]] const QString &name() const { return m_name; }
    [[nodiscard]] const QUuid &uuid() const { return m_uuid; }
    [[nodiscard]] const QList<QmlArgInfo *> &inputs() const { return m_inputs; }
    [[nodiscard]] const QList<QmlArgInfo *> &outputs() const { return m_outputs; }

private:
    QString m_name;
    QUuid m_uuid;
    QList<QmlArgInfo *> m_inputs;
    QList<QmlArgInfo *> m_outputs;
};
