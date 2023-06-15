#include "Counter.h"

Counter::Counter(QObject *parent)
        : QObject(parent), m_count(0) {
}

int Counter::count() const {
    return m_count;
}

void Counter::setCount(int count) {
    if (count != m_count) {
        m_count = count;
        emit countChanged(m_count);
    }
}

void Counter::increment() {
    setCount(m_count + 1);
}
