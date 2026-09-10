// SPDX-License-Identifier: Apache-2.0
// AI records table.
// ProTable / ProColumns 从 @ant-design/pro-components 导入。
import React from 'react';
import { message, Space, Tag } from 'antd';
import { FormattedMessage, useIntl } from 'react-intl';
import { ProTable } from '@ant-design/pro-components';
import type { ProColumns } from '@ant-design/pro-components';
import { withBase } from '../../../utils/basePath';

type AiAnalysisRecordItem = {
  analysisId: string;
  analysisTime: string;
  instance: string;
  parms: { key: string; value: string }[];
  status: string;
  failedLog: string;
  hasReport?: boolean;
};

const AiAnalysisRecordColumns: ProColumns<AiAnalysisRecordItem>[] = [
  {
    title: (
      <FormattedMessage id="pages.ai_performance_diagnose.analysisId" defaultMessage="analysis Id" />
    ),
    dataIndex: 'analysisId',
    valueType: 'textarea',
    width: 310,
    ellipsis: true,
    copyable: true,
  },
  {
    title: (
      <FormattedMessage id="pages.ai_performance_diagnose.analysisTime" defaultMessage="analysis Time" />
    ),
    dataIndex: 'analysisTime',
    valueType: 'textarea',
    width: 200,
    copyable: true,
  },
  {
    title: (
      <FormattedMessage id="pages.ai_performance_diagnose.instanceId_name" defaultMessage="instance id or name" />
    ),
    dataIndex: 'instance',
    valueType: 'textarea',
    width: 200,
    ellipsis: true,
    copyable: true,
    sorter: (a, b) => a.instance.localeCompare(b.instance),
  },
  {
    title: (
      <FormattedMessage id="pages.ai_performance_diagnose.analysis_parms" defaultMessage="analysis parms" />
    ),
    dataIndex: 'parms',
    valueType: 'textarea',
    width: 460,
    render: (_, record) => (
      <Space direction="horizontal" wrap style={{ width: '100%' }}>
        {record.parms.map(({ key, value }) => (
          <Tag
            key={key}
            style={{ maxWidth: 440, whiteSpace: 'normal', wordBreak: 'break-word', overflowWrap: 'anywhere' }}
          >
            {key}:{value}
          </Tag>
        ))}
      </Space>
    ),
  },
  {
    title: (
      <FormattedMessage id="pages.ai_performance_diagnose.analysis_status" defaultMessage="analysis status" />
    ),
    width: 160,
    dataIndex: 'status',
    initialValue: 'all',
    valueEnum: {
      分析中: { text: '分析中', status: 'Processing' },
      分析成功: { text: '分析成功', status: 'Success' },
      分析失败: { text: '分析失败', status: 'Error' },
      采集失败: { text: '采集失败', status: 'Error' },
      数据上传失败: { text: '数据上传失败', status: 'warning' },
      采集中: { text: '采集中', status: 'Processing' },
    },
  },
  {
    title: '操作',
    valueType: 'option',
    key: 'option',
    width: 140,
    fixed: 'right',
    render: (_, record) => {
      switch (record.status) {
        case '分析成功':
          return [
            <a
              key="showDetail"
              onClick={() => {
                window.open(withBase(`/ai_observable/result/Report?analysisId=${record?.analysisId}`));
              }}
            >
              <FormattedMessage id="pages.migrate.viewreport" defaultMessage="Viewing AI analysis result" />
            </a>,
            record.hasReport ? (
              <a
                key="htmlReport"
                onClick={() => {
                  window.open(
                    withBase(`/api/v1/app_observ/aiAnalysis/report?analysisId=${record?.analysisId}`),
                  );
                }}
              >
                <FormattedMessage id="pages.ai_performance_diagnose.html_report" defaultMessage="Report" />
              </a>
            ) : null,
          ];
        case '分析中':
        case '采集中':
          return [<></>];
        case '数据上传失败':
          return [
            <a
              key="faillog"
              onClick={() => {
                message.error('未获取到分析数据，请检查进程是否占用GPU或是否为python进程');
              }}
            >
              <FormattedMessage id="pages.ai_performance_diagnose.error_reason" defaultMessage="Error Reason" />
            </a>,
          ];
        case '采集失败':
        case '分析失败':
          return [
            <a
              key="faillog"
              onClick={() => {
                message.error(record?.failedLog);
              }}
            >
              <FormattedMessage id="pages.ai_performance_diagnose.error_reason" defaultMessage="Error Reason" />
            </a>,
          ];
        default:
          return [<></>];
      }
    },
  },
];

type AiRecordProps = {
  dataSource?: AiAnalysisRecordItem[];
  request?: any;
  setCurrentPage?: (p: number) => void;
  setPageSize?: (n: number) => void;
  total?: number;
};

export const AiRecordTable: React.FC<AiRecordProps> = (props) => {
  const intl = useIntl();
  return (
    <ProTable<AiAnalysisRecordItem>
      headerTitle={intl.formatMessage({
        id: 'pages.ai_performance_diagnose.ai_record',
        defaultMessage: 'ai_record',
      })}
      rowKey="analysisId"
      columns={AiAnalysisRecordColumns}
      dataSource={props.dataSource}
      search={false}
      scroll={{ x: 1200 }}
      tableLayout="fixed"
      pagination={{
        defaultPageSize: 10,
        total: props.total,
        onChange: (page: number, pageSize: number) => {
          props.setCurrentPage?.(page);
          props.setPageSize?.(pageSize);
        },
      }}
      request={props.request}
    />
  );
};
