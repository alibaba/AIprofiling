// SPDX-License-Identifier: Apache-2.0
import React from 'react';
import { Table, Tag, Space, Button, Input, Drawer, Popconfirm, message, Tooltip } from 'antd';
import type { TableProps } from 'antd';
import {
  SearchOutlined, CheckCircleFilled, CloseCircleFilled, LoadingOutlined, QuestionCircleOutlined,
} from '@ant-design/icons';
import { statusMeta } from '../i18n';
import { DeleteAnalysisRecord, GetExternalAnalyzer } from '../../../api';
import { withBase } from '../../../utils/basePath';

export type AnalysisRecord = {
  analysisId: string;
  analysisTime: string;
  instance: string;
  parms: Array<{ key: string; value: any }>;
  status: string;
  failedLog?: string;
};

interface Props {
  dataSource: AnalysisRecord[];
  total: number;
  loading?: boolean;
  onPageChange: (page: number, pageSize: number) => void;
  onRefresh?: () => void;
  onReuseParams?: (row: AnalysisRecord) => void;
}

// 参数序列化成截图里 "key=value" 灰色 tag 样式
function paramTags(parms: AnalysisRecord['parms']): Array<{ key: string; value: string }> {
  if (!parms?.length) return [];
  const out: Array<{ key: string; value: string }> = [];
  for (const p of parms) {
    const v = typeof p.value === 'object' ? JSON.stringify(p.value) : String(p.value ?? '');
    if (v === '' || v === 'undefined' || v === 'null') continue;
    // 把 "Params" 那个空格分隔的展开成 analysis_params=xxx 一串
    if (p.key === 'Params' && v.includes(' ')) {
      out.push({ key: 'analysis_params', value: v.split(/\s+/).filter(Boolean).join(',') });
    } else {
      out.push({ key: p.key, value: v });
    }
  }
  return out;
}

function StatusCell({ status }: { status: string }) {
  const m = statusMeta[status] ?? { text: status, kind: 'default' as const };
  // whiteSpace: nowrap 防止 icon+文字在窄列里换行，导致前一列 Space wrap
  // 的 tag 看起来被"遮住"。
  const wrap: React.CSSProperties = { whiteSpace: 'nowrap' };
  if (m.kind === 'success') {
    return <span style={wrap}><CheckCircleFilled style={{ color: '#52c41a', marginRight: 6 }} />{m.text}</span>;
  }
  if (m.kind === 'error') {
    return <span style={wrap}><CloseCircleFilled style={{ color: '#ff4d4f', marginRight: 6 }} />{m.text}</span>;
  }
  if (m.kind === 'running') {
    return <span style={wrap}><LoadingOutlined style={{ color: '#1677ff', marginRight: 6 }} />{m.text}</span>;
  }
  return <Tag>{m.text}</Tag>;
}

export const AiRecordTable: React.FC<Props> = ({
  dataSource, total, loading, onPageChange, onRefresh, onReuseParams,
}) => {
  const [filter, setFilter] = React.useState('');
  const [reportRow, setReportRow] = React.useState<AnalysisRecord | null>(null);
  const [failedRow, setFailedRow] = React.useState<AnalysisRecord | null>(null);

  const filtered = React.useMemo(() => {
    if (!filter.trim()) return dataSource;
    const kw = filter.trim().toLowerCase();
    return dataSource.filter(
      (r) =>
        r.analysisId?.toLowerCase().includes(kw) ||
        r.instance?.toLowerCase().includes(kw) ||
        r.status?.toLowerCase().includes(kw),
    );
  }, [dataSource, filter]);

  const isDone = (s: string) =>
    s === 'Success' || s === 'Succeeded' || s === 'Done' ||
    s === '分析成功' || s === '分析完成';
  // list_record reports queued rows as "排队中(第N位)", which carries the
  // position and so never matches a statusMeta key exactly.
  const isRunning = (s: string) =>
    statusMeta[s]?.kind === 'running' || s?.startsWith('排队中');

  // Entry point stays hidden unless an operator configured a companion
  // analyzer, so a default deployment shows nothing extra.
  const [analyzerUrl, setAnalyzerUrl] = React.useState('');
  // When the companion can pull traces server-to-server by analysisId
  // (same-host deployment), the button becomes a simple redirect. Otherwise
  // the browser downloads the trace and the user re-uploads it in the
  // companion — necessary for cross-origin / cross-host installs.
  const [importEnabled, setImportEnabled] = React.useState(false);
  React.useEffect(() => {
    GetExternalAnalyzer().then((r) => {
      if (r.code === 'ok' && r.data?.url) setAnalyzerUrl(r.data.url);
      if (r.code === 'ok' && r.data?.importEnabled) setImportEnabled(true);
    });
  }, []);

  const [deleting, setDeleting] = React.useState<string | null>(null);  const doDelete = async (row: AnalysisRecord) => {
    setDeleting(row.analysisId);
    const resp: any = await DeleteAnalysisRecord(row.analysisId);
    setDeleting(null);
    if (resp?.code === 'Success') {
      message.success(`已删除 ${row.analysisId.slice(0, 8)}…`);
      onRefresh?.();
    } else {
      message.error(`删除失败：${resp?.message ?? 'unknown'}`);
    }
  };

  const columns: TableProps<AnalysisRecord>['columns'] = [
    {
      title: '分析ID',
      dataIndex: 'analysisId',
      width: 300,
      render: (v) => (
        <span style={{ fontFamily: 'ui-monospace, Menlo, monospace', fontSize: 12, color: '#374151' }}>
          {v}
        </span>
      ),
    },
    {
      title: '分析时间',
      dataIndex: 'analysisTime',
      width: 180,
    },
    {
      title: '实例ID/名称',
      dataIndex: 'instance',
      width: 200,
    },
    {
      title: '分析参数',
      dataIndex: 'parms',
      width: 240,
      render: (parms: AnalysisRecord['parms']) => {
        const tags = paramTags(parms);
        if (!tags.length) return <span style={{ color: '#9ca3af' }}>—</span>;
        // 竖排：每个 tag 占一行，避免横向 wrap 把列高撑起来遮住相邻列。
        return (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            {tags.map((t, i) => (
              <span
                key={i}
                style={{
                  padding: '2px 8px',
                  background: '#f5f5f5',
                  border: '1px solid #e5e7eb',
                  borderRadius: 4,
                  fontFamily: 'ui-monospace, Menlo, monospace',
                  fontSize: 12,
                  color: '#4b5563',
                  whiteSpace: 'nowrap',
                  alignSelf: 'flex-start',
                }}
              >
                {t.key}={t.value}
              </span>
            ))}
          </div>
        );
      },
    },
    {
      title: '分析状态',
      dataIndex: 'status',
      width: 140,
      render: (s: string) => <StatusCell status={s} />,
    },
    {
      title: '操作',
      width: 380,
      fixed: 'right',
      render: (_, row) => (
        <Space size={4}>
          {isRunning(row.status) ? (
            <span style={{ color: '#1677ff', fontSize: 12 }}>
              <LoadingOutlined /> 采集分析中…
            </span>
          ) : isDone(row.status) ? (
            <>
              <Button type="link" size="small" onClick={() => setReportRow(row)}>
                查看报告
              </Button>
              {analyzerUrl && (
                <>
                  {importEnabled ? (
                    // Same-host companion: hand off the analysisId and let it
                    // pull the trace over the internal network. No client-side
                    // download; the companion appears with the trace already
                    // loaded.
                    <Button
                      type="link"
                      size="small"
                      href={`${analyzerUrl}/aiprof?importAnalysisId=${encodeURIComponent(row.analysisId)}`}
                      target="_blank"
                      rel="noreferrer"
                    >
                      云端 Trace 分析
                    </Button>
                  ) : (
                    <Button
                      type="link"
                      size="small"
                      onClick={() => {
                        // Two-step: trigger the download of the trace, then open
                        // the external analyzer in a new tab. The analyzer runs
                        // on HTTPS and this instance is HTTP-only, so browsers
                        // block any cross-origin fetch or postMessage that would
                        // carry the trace payload directly. A file handoff via
                        // the OS is the only channel that survives that gap.
                        const dl = document.createElement('a');
                        dl.href = withBase(`/api/v1/app_observ/aiAnalysis/trace?analysisId=${encodeURIComponent(row.analysisId)}`);
                        dl.download = `aiprof-trace-${row.analysisId}.json`;
                        document.body.appendChild(dl);
                        dl.click();
                        document.body.removeChild(dl);
                        window.open(analyzerUrl, '_blank', 'noopener');
                        message.info('已下载 trace，请在新窗口登录后上传该文件进行分析');
                      }}
                    >
                      云端 Trace 分析
                    </Button>
                  )}
                  <Tooltip
                    title={
                      importEnabled ? (
                        <>
                          点击后新开 {analyzerUrl} ，云端会自动拉取本条 trace 并进入分析。
                        </>
                      ) : (
                        <>
                          点击后浏览器会先下载本条记录的 trace JSON，并新开
                          {' '}{analyzerUrl}{' '}
                          。在云端环境登录后，手动上传刚下载的文件即可开始云端分析。
                        </>
                      )
                    }
                  >
                    <QuestionCircleOutlined style={{ color: '#9ca3af' }} />
                  </Tooltip>
                </>
              )}
              <Button type="link" size="small" onClick={() => onReuseParams?.(row)}>
                复用参数
              </Button>
            </>
          ) : (
            <>
              <Button type="link" size="small" onClick={() => onReuseParams?.(row)}>
                重试
              </Button>
              <Button
                type="link"
                size="small"
                danger
                disabled={!row.failedLog}
                onClick={() => setFailedRow(row)}
              >
                失败原因
              </Button>
            </>
          )}
          <Popconfirm
            title="确认删除该分析记录？"
            description="将删除结果目录和内存记录，不可恢复。"
            okText="删除"
            okButtonProps={{ danger: true }}
            cancelText="取消"
            disabled={isRunning(row.status)}
            onConfirm={() => doDelete(row)}
          >
            <Button
              type="link"
              size="small"
              danger
              loading={deleting === row.analysisId}
              disabled={isRunning(row.status)}
              title={isRunning(row.status) ? '任务正在采集中，等结束后再删除' : ''}
            >
              删除
            </Button>
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <div>
      <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 12 }}>
        <div style={{ fontWeight: 500, fontSize: 15 }}>分析记录</div>
        <Space>
          <Input
            allowClear
            size="small"
            prefix={<SearchOutlined />}
            placeholder="搜索 ID / 实例 / 状态"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            style={{ width: 260 }}
          />
          {onRefresh && <Button size="small" onClick={onRefresh}>刷新</Button>}
        </Space>
      </div>
      <Table<AnalysisRecord>
        rowKey="analysisId"
        size="middle"
        columns={columns}
        dataSource={filtered}
        loading={loading}
        scroll={{ x: 1200 }}
        pagination={{
          total,
          showSizeChanger: true,
          onChange: onPageChange,
          pageSizeOptions: [10, 20, 50],
          showTotal: (t) => `共 ${t} 条`,
        }}
      />

      {/* 内嵌报告 Drawer —— iframe 复用 /aiprof/result 页 */}
      <Drawer
        title={
          <span>
            分析报告
            <span style={{ marginLeft: 12, color: '#6b7280', fontSize: 12, fontFamily: 'ui-monospace, Menlo, monospace' }}>
              {reportRow?.analysisId}
            </span>
          </span>
        }
        placement="right"
        width="90%"
        open={!!reportRow}
        onClose={() => setReportRow(null)}
        destroyOnClose
        bodyStyle={{ padding: 0 }}
        extra={
          reportRow && (
            <Button
              type="link"
              href={withBase(`/aiprof/result?analysisId=${encodeURIComponent(reportRow.analysisId)}`)}
              target="_blank"
              rel="noreferrer"
            >
              在新窗口打开
            </Button>
          )
        }
      >
        {reportRow && (
          <iframe
            key={reportRow.analysisId}
            title="analysis-report"
            src={withBase(`/aiprof/result?analysisId=${encodeURIComponent(reportRow.analysisId)}&embed=1`)}
            style={{ width: '100%', height: '100%', border: 'none', minHeight: 'calc(100vh - 64px)' }}
          />
        )}
      </Drawer>

      {/* 失败原因 Drawer */}
      <Drawer
        title="失败原因"
        placement="right"
        width={640}
        open={!!failedRow}
        onClose={() => setFailedRow(null)}
      >
        <pre style={{
          whiteSpace: 'pre-wrap',
          fontFamily: 'ui-monospace, Menlo, monospace',
          fontSize: 12,
          color: '#b91c1c',
          margin: 0,
        }}>
          {failedRow?.failedLog || '（无日志）'}
        </pre>
      </Drawer>
    </div>
  );
};

export default AiRecordTable;
