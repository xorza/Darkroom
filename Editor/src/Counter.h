#pragma once

#include <QObject>

class Counter : public QObject {
Q_OBJECT

    Q_PROPERTY(int count READ count WRITE setCount NOTIFY countChanged)

public:
    explicit Counter(QObject *parent = nullptr);

    int count() const;

    void setCount(int count);

signals:

    void countChanged(int count);

public slots:

    void increment();

private:
    int m_count;
};
