#pragma once

#include <QObject>

#include "function_info.hpp"


class AppModel : public QObject {
Q_OBJECT

    Q_PROPERTY(QList<QmlFunctionInfo *> functions READ functions CONSTANT)

private:
    QList<QmlFunctionInfo *> m_functions;

public:
    AppModel();

    ~AppModel() override;

    [[nodiscard]] const QList<QmlFunctionInfo *> &functions() const { return m_functions; }
};
