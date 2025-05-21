#pragma once

#include "FuncController.hpp"

#include <QtCore>
#include <QQuickItem>


class FuncLibrary : public QObject {
Q_OBJECT

    Q_PROPERTY(QList<FuncController *> funcs READ funcs NOTIFY funcsChanged)



public:
    explicit FuncLibrary(QObject *parent);

    ~FuncLibrary() override = default;

    [[nodiscard]] const QList<FuncController *> &funcs() const {
        return m_funcs;
    }

signals:
    void funcsChanged();

public slots:

private:
    QList<FuncController *> m_funcs;
};
