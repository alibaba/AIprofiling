// SPDX-License-Identifier: Apache-2.0
import React, { useEffect, useMemo, useState } from 'react';
import {
  Layout, Card, Tabs, Space, Button, Descriptions, Row, Col, Empty, Alert, Spin, Table, Tag, Progress, Select,
} from 'antd';
import type { TableProps } from 'antd';
import { ArrowLeftOutlined, FullscreenOutlined, FullscreenExitOutlined } from '@ant-design/icons';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { GetAIQueryResult } from '../../../api';
import { withBase } from '../../../utils/basePath';
import ProfileTab from './ProfileTab';
import AppFooter from '../components/AppFooter';

const { Header, Content } = Layout;

// server 的 Analysis_Summary.json 结构（真实 shape）：
// { code, message, data: "<JSON string>" | object }
// data 反序列化后：
//   {
//     aggregationUrl,
//     "<pid>:[GPU0] Offline Profiling Task": {
//        traceUrl, traceFileSize (MB),
//        overview: {
//           detail: {
//              kernel_details: { <kernel_name>: {...} },
//              tensorCores_usage: { service_time, unused_time },
//              kernel_stistics: { Computation, Memory, Communication },
//              memory_stistics: { Allocated, Reserved, Time },
//           },
//           summary: {
//              conclusion,
//              device_info: [{ device_name, memory_size, memory_used }],
//              GPU_utilization: { GPU0: { SM_utilization, active_blocks_per_SM, ... }, GPU_total: {...} },
//              execution_delay: { python_function, cpu_op, overhead, kernel, cuda_runtime, total },
//           },
//        },
//        stepInfo, scenario, ...
//     }
//   }
type KernelDetail = {
  kernel_name: string;
  type_of_operation?: string;
  run_times?: number;
  use_tensorCore?: string;
  total_delay_us?: number;
  max_delay_us?: number;
  avg_delay_us?: number;
  min_delay_us?: number;
  block?: string;
  grid?: string;
  SM_utilization?: number;
  active_blocks_per_SM?: number;
  shared_memory_size?: number;
  registers_per_thread?: number;
};

type SessionData = {
  traceUrl?: string;
  traceFileSize?: number;
  mmSnapUrl?: string;
  memSummary?: string;
  memOom?: { summary?: string; oomItem?: any[] };
  memspikes?: Array<{ time?: string; size?: number; addr?: string; stack?: string }>;
  overview?: {
    detail?: {
      kernel_details?: Record<string, KernelDetail>;
      tensorCores_usage?: { service_time?: number; unused_time?: number };
      kernel_stistics?: { Computation?: number; Memory?: number; Communication?: number };
      memory_stistics?: { Allocated?: number[]; Reserved?: number[]; Time?: number[] };
    };
    summary?: {
      conclusion?: string;
      device_info?: Array<{ device_name: string; memory_size?: number; memory_used?: number }>;
      GPU_utilization?: Record<string, { GPU_utilization?: number; SM_utilization?: number; active_blocks_per_SM?: number; active_warps_per_SM?: number }>;
      execution_delay?: { python_function?: number; cpu_op?: number; overhead?: number; kernel?: number; cuda_runtime?: number; total?: number };
    };
  };
};

function parsePayload(raw: any): { aggregationUrl?: string; sessions: Array<{ label: string; pid?: string; data: SessionData }> } {
  const outer = typeof raw === 'string' ? JSON.parse(raw) : (raw || {});
  const sessions: Array<{ label: string; pid?: string; data: SessionData }> = [];
  let aggregationUrl: string | undefined = outer.aggregationUrl;
  for (const [k, v] of Object.entries(outer)) {
    if (k === 'aggregationUrl' || v == null || typeof v !== 'object') continue;
    // key 形如 "2815111:[GPU0] Offline Profiling Task"
    const pid = k.match(/^(\d+):/)?.[1];
    sessions.push({ label: k, pid, data: v as SessionData });
  }
  return { aggregationUrl, sessions };
}

const fmtUs = (v?: number) => {
  if (v == null) return '-';
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(2)} s`;
  if (v >= 1_000) return `${(v / 1_000).toFixed(2)} ms`;
  return `${v.toFixed(2)} µs`;
};
const fmtPct = (v?: number) => (v == null ? '-' : `${(v * 100).toFixed(2)}%`);
const fmtMB = (v?: number) => (v == null ? '-' : `${v.toFixed(2)} MB`);

const ResultPage: React.FC = () => {
  const nav = useNavigate();
  const [sp] = useSearchParams();
  const analysisId = sp.get('analysisId') ?? '';
  const embed = sp.get('embed') === '1';
  const [loading, setLoading] = useState(true);
  const [rawPayload, setRawPayload] = useState<any>(null);
  const [error, setError] = useState<string>('');

  useEffect(() => {
    if (!analysisId) {
      setError('缺少 analysisId 参数');
      setLoading(false);
      return;
    }
    setLoading(true);
    GetAIQueryResult(analysisId).then((resp: any) => {
      setLoading(false);
      // server 返回 { code:'Success', message, data:'<JSON string>' } — data 是字符串！
      if (resp && (resp.code === 'Success' || resp.code === 'success') && resp.data != null) {
        try {
          setRawPayload(resp.data);
        } catch (e: any) {
          setError(`解析结果失败：${e.message ?? e}`);
        }
      } else {
        setError(resp?.message || '加载分析结果失败');
      }
    });
  }, [analysisId]);

  const { sessions } = useMemo(() => {
    if (!rawPayload) return { sessions: [] as any[] };
    try { return parsePayload(rawPayload); } catch { return { sessions: [] as any[] }; }
  }, [rawPayload]);

  // 多 PID 时让用户选；默认挑 traceFileSize 最小那个 —— Perfetto 加载最快
  const [activeIdx, setActiveIdx] = useState<number>(0);
  useEffect(() => {
    if (!sessions.length) return;
    let minIdx = 0;
    let minSize = Number.POSITIVE_INFINITY;
    sessions.forEach((s: any, i: number) => {
      const sz = s.data?.traceFileSize ?? Number.POSITIVE_INFINITY;
      if (sz < minSize) { minSize = sz; minIdx = i; }
    });
    setActiveIdx(minIdx);
  }, [sessions]);

  const session = sessions[activeIdx] || sessions[0];
  const overview = session?.data?.overview;
  const summary = overview?.summary;
  const detail = overview?.detail;
  const kernels: KernelDetail[] = detail?.kernel_details ? Object.values(detail.kernel_details) as KernelDetail[] : [];
  // server 返回的 traceUrl 走的是 legacy /resource/... 静态路径 —— 但 Express 已经不映射它了。
  // 用 server 现在真正暴露的 /api/v1/.../trace?analysisId=&pid= 端点重建 URL。
  const traceAbs = analysisId
    ? withBase(`/api/v1/app_observ/aiAnalysis/trace?analysisId=${encodeURIComponent(analysisId)}${session?.pid ? `&pid=${encodeURIComponent(session.pid)}` : ''}`)
    : '';

  const totalKernelUs = kernels.reduce((s, k) => s + (k.total_delay_us ?? 0), 0);
  const execDelay = summary?.execution_delay;

  return (
    <Layout style={{ minHeight: '100vh' }}>
      {!embed && (
        <Header
          style={{
            background: '#fff',
            borderBottom: '1px solid #f0f0f0',
            padding: '0 24px',
            display: 'flex',
            alignItems: 'center',
            gap: 12,
          }}
        >
          <Button type="text" icon={<ArrowLeftOutlined />} onClick={() => nav('/aiprof')}>
            返回
          </Button>
          <span className="aiprof-logo">
            <span className="mark">A</span>
            AIProf
          </span>
          <span style={{ color: '#6b7280', fontSize: 13 }}>会话</span>
          <code style={{ fontSize: 12, color: '#374151' }}>{analysisId || '—'}</code>
          {sessions.length > 1 && (
            <>
              <span style={{ color: '#6b7280', fontSize: 13, marginLeft: 16 }}>PID Session</span>
              <Select
                size="small"
                style={{ minWidth: 380 }}
                value={activeIdx}
                onChange={(v) => setActiveIdx(v)}
                options={sessions.map((s: any, i: number) => ({
                  value: i,
                  label: (
                    <span>
                      PID <code style={{ fontSize: 12 }}>{s.pid || '?'}</code>
                      <span style={{ color: '#6b7280', marginLeft: 8, fontSize: 12 }}>
                        trace {fmtMB(s.data?.traceFileSize)}
                      </span>
                    </span>
                  ),
                }))}
              />
            </>
          )}
        </Header>
      )}
      <Content style={{ padding: embed ? 12 : 20 }}>
        {loading && <div style={{ textAlign: 'center', padding: 60 }}><Spin /></div>}
        {!loading && error && <Alert type="error" showIcon message={error} style={{ marginBottom: 16 }} />}
        {!loading && !error && !session && (
          <Empty description="该分析记录暂无可解析的结果数据（Analysis_Summary.json 缺失或为空）" />
        )}

        {!loading && !error && session && (
          <Tabs
            defaultActiveKey="profile"
            items={[
              {
                key: 'profile',
                label: 'AI 性能分析',
                children: <ProfileTab session={session as any} analysisId={analysisId} />,
              },
              {
                key: 'trace',
                label: 'Perfetto 追踪',
                children: traceAbs ? (
                  <Card bordered={false} size="small">
                    <Space direction="vertical" size={12} style={{ width: '100%' }}>
                      <Alert
                        type="info"
                        showIcon
                        message={
                          <span>
                            Chrome / Perfetto 兼容 trace：{fmtMB(session.data.traceFileSize)}
                            {session.pid ? <> · PID <code>{session.pid}</code></> : null}
                          </span>
                        }
                      />
                      <Space>
                        <Button type="primary" href={traceAbs} target="_blank" rel="noreferrer" download>
                          下载 trace JSON
                        </Button>
                      </Space>
                      <PerfettoEmbed traceUrl={traceAbs} title={session.label} />
                    </Space>
                  </Card>
                ) : (
                  <Empty description="该会话未产出 Chrome/Perfetto trace" />
                ),
              },
            ]}
          />
        )}
      </Content>
      <AppFooter />
    </Layout>
  );
};

export default ResultPage;

const PERFETTO_ORIGIN = 'https://ui.perfetto.dev';

export const PerfettoEmbed: React.FC<{ traceUrl: string; title?: string }> = ({ traceUrl, title }) => {
  const iframeRef = React.useRef<HTMLIFrameElement | null>(null);
  const [status, setStatus] = React.useState<'idle' | 'fetching' | 'waiting' | 'ready' | 'error'>('idle');
  const [errMsg, setErrMsg] = React.useState<string>('');
  const [progress, setProgress] = React.useState<{ received: number; total: number } | null>(null);
  const [fullscreen, setFullscreen] = React.useState(false);
  const bufferRef = React.useRef<ArrayBuffer | null>(null);
  const pingTimerRef = React.useRef<any>(null);
  const ORIGIN = PERFETTO_ORIGIN;

  // ESC 退出全屏
  React.useEffect(() => {
    if (!fullscreen) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setFullscreen(false); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [fullscreen]);

  React.useEffect(() => {
    let cancelled = false;
    setStatus('fetching');
    setErrMsg('');
    setProgress(null);
    bufferRef.current = null;

    (async () => {
      try {
        const r = await fetch(traceUrl);
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        const total = Number(r.headers.get('content-length') || 0);
        // 边下载边显示进度
        if (r.body && total > 0) {
          const reader = r.body.getReader();
          const chunks: Uint8Array[] = [];
          let received = 0;
          for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            if (cancelled) return;
            chunks.push(value);
            received += value.byteLength;
            setProgress({ received, total });
          }
          const merged = new Uint8Array(received);
          let off = 0;
          for (const c of chunks) { merged.set(c, off); off += c.byteLength; }
          bufferRef.current = merged.buffer;
        } else {
          bufferRef.current = await r.arrayBuffer();
        }
        if (cancelled) return;
        setStatus('waiting');
      } catch (e: any) {
        if (cancelled) return;
        setErrMsg(e?.message || String(e));
        setStatus('error');
      }
    })();
    return () => { cancelled = true; };
  }, [traceUrl]);

  // 一旦 buffer 就绪并且 iframe 已挂载，向 iframe 循环发 PING；收到 PONG 后推送 trace。
  React.useEffect(() => {
    if (status !== 'waiting') return;
    const iframe = iframeRef.current;
    if (!iframe) return;

    const onMessage = (ev: MessageEvent) => {
      if (ev.origin !== ORIGIN) return;
      if (ev.data !== 'PONG') return;
      // 收到 PONG，停 ping，注入 trace
      if (pingTimerRef.current) { clearInterval(pingTimerRef.current); pingTimerRef.current = null; }
      const buf = bufferRef.current;
      const win = iframe.contentWindow;
      if (!buf || !win) return;
      win.postMessage(
        {
          perfetto: {
            buffer: buf,
            title: title || 'AIProf trace',
            fileName: (title || 'aiprof') + '.json',
          },
        },
        ORIGIN,
      );
      setStatus('ready');
    };
    window.addEventListener('message', onMessage);

    // Perfetto UI iframe 就绪后才响应 PONG；持续 ping 到收到为止（每 250ms 一次）
    pingTimerRef.current = setInterval(() => {
      const win = iframe.contentWindow;
      if (win) win.postMessage('PING', ORIGIN);
    }, 250);

    return () => {
      window.removeEventListener('message', onMessage);
      if (pingTimerRef.current) { clearInterval(pingTimerRef.current); pingTimerRef.current = null; }
    };
  }, [status, title]);

  const fmtBytes = (n: number) => {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / 1024 / 1024).toFixed(2)} MB`;
  };

  return (
    <div
      style={
        fullscreen
          ? {
              position: 'fixed',
              inset: 0,
              zIndex: 1000,
              background: '#fff',
              display: 'flex',
              flexDirection: 'column',
            }
          : { position: 'relative' }
      }
    >
      <div
        style={{
          position: 'absolute',
          top: 8,
          right: 12,
          zIndex: 3,
          display: 'flex',
          gap: 4,
        }}
      >
        <Button
          size="small"
          type="default"
          icon={fullscreen ? <FullscreenExitOutlined /> : <FullscreenOutlined />}
          onClick={() => setFullscreen((v) => !v)}
        >
          {fullscreen ? '退出全屏 (Esc)' : '全屏展开'}
        </Button>
      </div>
      {status !== 'ready' && (
        <div
          style={{
            position: 'absolute',
            inset: 0,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            background: 'rgba(255,255,255,0.9)',
            zIndex: 2,
            pointerEvents: status === 'error' ? 'auto' : 'none',
          }}
        >
          {status === 'error' ? (
            <Alert
              type="error"
              showIcon
              message="加载 trace 失败"
              description={errMsg + '（可点上方"下载 trace"按钮手动打开）'}
              style={{ maxWidth: 480 }}
            />
          ) : (
            <div style={{ color: '#374151', fontSize: 13, textAlign: 'center' }}>
              <Spin />
              <div style={{ marginTop: 12 }}>
                {status === 'fetching' && (progress
                  ? `正在下载 trace… ${fmtBytes(progress.received)} / ${fmtBytes(progress.total)}（${((progress.received / progress.total) * 100).toFixed(0)}%）`
                  : '正在从服务器下载 trace 数据…')}
                {status === 'waiting' && '数据已下载，正在等待 Perfetto UI 就绪…（首次加载 WASM 可能需要 5-30 秒）'}
              </div>
            </div>
          )}
        </div>
      )}
      <iframe
        ref={iframeRef}
        title="perfetto-embed"
        src={ORIGIN + '/?hideSidebar=true'}
        style={
          fullscreen
            ? {
                flex: 1,
                width: '100%',
                border: 'none',
                display: 'block',
              }
            : {
                width: '100%',
                height: 'calc(100vh - 260px)',
                minHeight: 480,
                border: '1px solid #e5e7eb',
                borderRadius: 6,
                display: 'block',
              }
        }
      />
    </div>
  );
};

const Statistic: React.FC<{ title: string; value: string | number; unit?: string }> = ({ title, value, unit }) => (
  <div>
    <div style={{ color: '#6b7280', fontSize: 12, marginBottom: 4 }}>{title}</div>
    <div style={{ fontSize: 22, fontWeight: 500, color: '#111827' }}>
      {value} <span style={{ fontSize: 13, color: '#9ca3af', fontWeight: 400 }}>{unit}</span>
    </div>
  </div>
);

const DelayBars: React.FC<{ data: NonNullable<SessionData['overview']>['summary'] extends infer S ? S extends { execution_delay?: infer D } ? D : any : any }> = ({ data }) => {
  const items: Array<{ k: string; v: number; label: string }> = [];
  const push = (k: string, label: string) => {
    const v = (data as any)?.[k];
    if (typeof v === 'number' && v > 0) items.push({ k, v, label });
  };
  push('python_function', 'Python 函数');
  push('cuda_runtime',    'CUDA runtime');
  push('cpu_op',          'CPU 算子');
  push('kernel',          'GPU Kernel');
  push('overhead',        'Overhead');
  const max = Math.max(...items.map(i => i.v), 1);
  return (
    <div>
      {items.map(it => (
        <div key={it.k} style={{ display: 'grid', gridTemplateColumns: '110px 1fr 120px', alignItems: 'center', gap: 8, margin: '6px 0' }}>
          <div style={{ color: '#4b5563', fontSize: 13 }}>{it.label}</div>
          <div style={{ height: 12, background: '#f5f5f5', borderRadius: 3, overflow: 'hidden' }}>
            <div style={{
              height: '100%',
              width: `${(it.v / max) * 100}%`,
              background: 'linear-gradient(90deg, #69b1ff, #1677ff)',
            }} />
          </div>
          <div style={{ textAlign: 'right', fontSize: 12, color: '#374151', fontFamily: 'ui-monospace, Menlo, monospace' }}>
            {fmtUs(it.v)}
          </div>
        </div>
      ))}
      {(data as any)?.total != null && (
        <div style={{ marginTop: 8, color: '#6b7280', fontSize: 12 }}>
          总耗时：{fmtUs((data as any).total)}
        </div>
      )}
    </div>
  );
};

const KernelTable: React.FC<{ kernels: KernelDetail[]; totalUs: number }> = ({ kernels, totalUs }) => {
  const cols: TableProps<KernelDetail>['columns'] = [
    {
      title: 'Kernel',
      dataIndex: 'kernel_name',
      ellipsis: true,
      render: (v: string) => (
        <span style={{ fontFamily: 'ui-monospace, Menlo, monospace', fontSize: 12 }} title={v}>{v}</span>
      ),
    },
    {
      title: '调用次数',
      dataIndex: 'run_times',
      width: 90,
      sorter: (a, b) => (a.run_times ?? 0) - (b.run_times ?? 0),
      align: 'right',
    },
    {
      title: '总耗时 (µs)',
      dataIndex: 'total_delay_us',
      width: 130,
      align: 'right',
      defaultSortOrder: 'descend',
      sorter: (a, b) => (a.total_delay_us ?? 0) - (b.total_delay_us ?? 0),
      render: (v: number) => v?.toFixed(3),
    },
    { title: '占比',       width: 90,  align: 'right', render: (_, r) => `${((r.total_delay_us ?? 0) / (totalUs || 1) * 100).toFixed(2)}%` },
    { title: '均值 (µs)',  dataIndex: 'avg_delay_us', width: 100, align: 'right', render: (v: number) => v?.toFixed(3) },
    { title: '最大 (µs)',  dataIndex: 'max_delay_us', width: 100, align: 'right', render: (v: number) => v?.toFixed(3) },
    { title: 'SM 利用率',  dataIndex: 'SM_utilization', width: 100, align: 'right', render: (v: number) => fmtPct(v) },
    {
      title: 'Tensor Core',
      dataIndex: 'use_tensorCore',
      width: 100,
      align: 'center',
      render: (v: string) => v === '是' ? <Tag color="green">是</Tag> : <Tag>{v || '否'}</Tag>,
    },
    { title: 'Grid',  dataIndex: 'grid',  width: 100 },
    { title: 'Block', dataIndex: 'block', width: 100 },
  ];
  return (
    <>
      <Card size="small" bordered={false} style={{ marginBottom: 12 }}>
        <Row gutter={16}>
          <Col span={8}><Statistic title="Kernel 种类" value={kernels.length} /></Col>
          <Col span={8}><Statistic title="总耗时" value={fmtUs(totalUs)} /></Col>
          <Col span={8}><Statistic title="总调用次数" value={kernels.reduce((s, k) => s + (k.run_times ?? 0), 0)} /></Col>
        </Row>
      </Card>
      <Table<KernelDetail>
        rowKey="kernel_name"
        size="small"
        columns={cols}
        dataSource={kernels}
        pagination={{ pageSize: 20, showSizeChanger: true, pageSizeOptions: [10, 20, 50, 100] }}
        scroll={{ x: 1200 }}
      />
    </>
  );
};
