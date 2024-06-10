#pragma once

#include <QtCore>
#include <QQuickItem>

class FuncLibrary;

class FuncController : public QObject {
Q_OBJECT

public:
    explicit FuncController(FuncLibrary *parent = nullptr);

    ~FuncController() override = default;

    [[nodiscard]] QString name() const {
        return m_name;
    }

signals:

public slots:


private:
    QString m_name;
};

