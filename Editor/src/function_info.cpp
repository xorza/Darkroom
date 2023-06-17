#include "function_info.hpp"

#include "graph_c_api.hpp"

QmlFunctionInfo::QmlFunctionInfo(
        const graph_c_api::FunctionInfo &func,
        QObject *parent
) : QObject(parent) {
    m_name = QString::fromStdString(func.m_name);
    m_uuid = QUuid::fromRfc4122(QByteArray::fromRawData((char *) func.m_uuid, 16));

    for (const auto &input: func.m_inputs) {
        m_inputs.append(
                new QmlArgInfo(
                        QString::fromStdString(input.m_name),
                        input.m_type,
                        this
                ));
    }

    for (const auto &output: func.m_outputs) {
        m_outputs.append(
                new QmlArgInfo(
                        QString::fromStdString(output.m_name),
                        output.m_type,
                        this
                ));
    }
}
