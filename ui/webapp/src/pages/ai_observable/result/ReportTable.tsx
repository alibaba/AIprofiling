
import React from 'react';
import { ProTable } from '@ant-design/pro-components';
import { Button } from "antd";

import type { ProColumns } from '@ant-design/pro-components';
import { withBase } from '../../../utils/basePath';



type AiAnalysisRecordItem = {
    pid: string;
    GPUpid: string;
    comm: string;
    analysisId: string;
};



interface ReportTableProps {
    dataSource?: AiAnalysisRecordItem[];
    request?: any;
    setCurrentPage?: any;
    setPageSize?: any;
    total?: any
};

const ReportTable: React.FC<ReportTableProps> = ({ dataSource, total }) => {
    const AiAnalysisRecordColumns: ProColumns<AiAnalysisRecordItem>[] = [
        {
            title: '进程ID',
            dataIndex: 'pid',
            valueType: 'textarea',
            width: 250,
            ellipsis: true
        },
        {
            title: 'GPU ID',
            dataIndex: 'GPUpid',
            valueType: 'textarea',
            width: 250,
            ellipsis: true
        },
        {
            title: '进程名',
            dataIndex: 'comm',
            valueType: 'textarea',
            ellipsis: true
        },
        {
            title: '设备名',
            dataIndex: 'device',
            valueType: 'textarea',
            ellipsis: true
        },
        {
            title: '显存总量/MiB',
            dataIndex: 'total_memory',
            valueType: 'textarea',
            ellipsis: true
        },
        {
            title: '显存占用',
            dataIndex: 'memory_rate',
            valueType: 'textarea',
            ellipsis: true
        },
        {
            title: '查看单进程分析报告',
            valueType: 'option',
            key: 'option',
            width: 180, // 稍微加宽以容纳按钮
            align: 'center', // 居中更醒目
            render: (_, record) => {
                return (
                    <Button
                        type="primary" // 主色按钮更突出
                        size="small"
                        // icon={<SearchOutlined />} // 添加图标增强识别
                        onClick={() => {
                            // navigate(`/ai_observable/result/?analysisId=${record?.analysisId}&pid=${record?.pid}`);
                            window.location.href = withBase(`/ai_observable/result/?analysisId=${record?.analysisId}&pid=${record?.pid}`);
                        }}
                    >
                        查看报告
                    </Button>
                );
            }
        },
    ];

    return (
        <ProTable
            headerTitle={'采集的进程列表'}
            rowKey={(record) => `${record.analysisId}-${record.pid}-${record.GPUpid || 'nogpu'}`}
            columns={AiAnalysisRecordColumns}
            dataSource={dataSource}
            search={false}
            pagination={{
                defaultPageSize: 10,
            }}
        />
    );
};
export default ReportTable;
