#include "NodeController.hpp"


void ArgumentController::setName(const QString &name) {
    if (m_name == name) {
        return;
    }

    m_name = name;
    emit nameChanged();

}

void NodeController::setName(const QString &name)  {
    if (m_name == name) {
        return;
    }

    m_name = name;
    emit nameChanged();
}
