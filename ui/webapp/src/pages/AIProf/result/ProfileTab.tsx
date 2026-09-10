// SPDX-License-Identifier: Apache-2.0
// AI 性能分析 Tab —— 按参考图重设计
//   1) 顶部 左侧设备/利用率卡 + 右侧 GPU/CPU 函数调用耗时横向条形图
//   2) Kernel 分析：三块环形图（GPU内核类别 / Tensor Cores / 函数调用 Top）
//   3) Top 内核函数表：筛选/搜索/Top N + 展开详情
import React, { useEffect, useMemo, useState } from 'react';
import { Card, Row, Col, Alert, Input, Select, AutoComplete, Button, Space, Tag, Table, Empty, Modal, Form, message, Spin, Typography } from 'antd';
import type { TableProps } from 'antd';
import { ExperimentOutlined, DownOutlined, UpOutlined, SettingOutlined, SaveOutlined, EyeOutlined, EyeInvisibleOutlined, SwapOutlined } from '@ant-design/icons';
import ReactECharts from 'echarts-for-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { withBase } from '../../../utils/basePath';

// —— 类型 —— //
export type KernelDetail = {
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

export type ExecDelay = Record<string, number | undefined> & { total?: number };

export type FlameNode = {
  name: string;
  value: number;
  children?: FlameNode[];
};

// —— 工具 —— //
const fmtUs = (v?: number) => {
  if (v == null || !isFinite(v)) return '-';
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(2)} s`;
  if (v >= 1_000) return `${(v / 1_000).toFixed(2)} ms`;
  return `${v.toFixed(2)} µs`;
};
// 后端返回的利用率已是百分比刻度（例如 67.255 表示 67.255%），前端不再 ×100
const fmtPctNum = (v?: number, digits = 1) =>
  v == null || !isFinite(Number(v)) ? '-' : `${Number(v).toFixed(digits)}%`;
// SM 活跃度等字段可能是数字，也可能是后端占位符 "N/A"（cuprof 场景常见），
// 裸调 toFixed 会崩掉整个 tab；这里统一走安全格式化。
const fmtNum = (v: unknown, digits = 2): string => {
  const n = typeof v === 'number' ? v : Number(v);
  return Number.isFinite(n) ? n.toFixed(digits) : '-';
};

// execution_delay 字段中文标签
const DELAY_LABEL: Record<string, string> = {
  cpu_op: 'cpu_op',
  cuda_runtime: 'cuda_runtime',
  kernel: 'kernel',
  gpu_memcpy: 'gpu_memcpy',
  gpu_memset: 'gpu_memset',
  cuda_driver: 'cuda_driver',
  python_function: 'python_function',
  overhead: 'overhead',
};
const DELAY_COLORS = ['#6366f1', '#14b8a6', '#c026d3', '#a16207', '#3b82f6', '#0ea5e9', '#f59e0b', '#8b5cf6', '#ec4899'];

// kernel 类型分类（从 type_of_operation 推断 → 中文 tag）
const KERNEL_TYPE_TAG: Record<string, { label: string; color: string }> = {
  computation: { label: '计算', color: 'blue' },
  memory:      { label: '内存', color: 'orange' },
  communication:{ label: '通信', color: 'purple' },
  sync:        { label: '同步', color: 'red' },
  runtime:     { label: '运行', color: 'default' },
};
const classifyKernel = (k: KernelDetail): keyof typeof KERNEL_TYPE_TAG => {
  const t = (k.type_of_operation || '').toLowerCase();
  if (t.includes('comm')) return 'communication';
  if (t.includes('mem')) return 'memory';
  if (t.includes('sync')) return 'sync';
  if (t.includes('runtime')) return 'runtime';
  // heuristic on name
  const n = (k.kernel_name || '').toLowerCase();
  if (n.includes('memcpy') || n.includes('memset')) return 'memory';
  if (n.includes('nccl') || n.includes('allreduce') || n.includes('broadcast')) return 'communication';
  if (n.includes('barrier') || n.includes('sync')) return 'sync';
  return 'computation';
};

// —— 组件 —— //
type Props = {
  analysisId?: string;
  session: {
    label: string;
    pid?: string;
    data: {
      overview?: {
        summary?: {
          conclusion?: string;
          device_info?: Array<{ device_name?: string; memory_size?: number; memory_used?: number }>;
          GPU_utilization?: Record<string, { GPU_utilization?: number; SM_utilization?: number; active_blocks_per_SM?: number; active_warps_per_SM?: number }>;
          execution_delay?: ExecDelay;
        };
        detail?: {
          kernel_details?: Record<string, KernelDetail>;
          tensorCores_usage?: { service_time?: number; unused_time?: number };
          kernel_stistics?: { Computation?: number; Memory?: number; Communication?: number };
          memory_stistics?: { Allocated?: number[]; Reserved?: number[]; Time?: number[] };
        };
      };
      raw_device_props?: Array<{ id?: number; name?: string; totalGlobalMem?: number; memoryUsed?: number; multiProcessorCount?: number; cudaCores?: number }>;
      mmSnapUrl?: string;
      memSummary?: string;
      memspikes?: any[];
      memOom?: { oomItem?: any[] };
      stackInfo?: FlameNode | Record<string, never>;
      pythonHotFrames?: Array<{ name: string; self_us: number }>;
    };
  };
};

export const ProfileTab: React.FC<Props> = ({ session, analysisId }) => {
  const overview = session?.data?.overview;
  const summary = overview?.summary;
  const detail = overview?.detail;
  const kernels: KernelDetail[] = detail?.kernel_details ? Object.values(detail.kernel_details) : [];
  const devProps = session?.data?.raw_device_props?.[0] || {};
  const devInfo = summary?.device_info?.[0] || {};
  const gpuTotal = summary?.GPU_utilization?.GPU_total || {};

  // —— 设备信息 —— //
  const deviceId = `GPU-${devProps.id ?? 0}`;
  const deviceName = devProps.name || devInfo.device_name || 'Unknown';
  const memSize = devProps.totalGlobalMem ?? devInfo.memory_size;
  const cudaCores = devProps.cudaCores ?? '-';

  return (
    <>
      {/* AI 分析结论卡 + Profiling Agent 入口 */}
      <ProfilingAgentPanel
        analysisId={analysisId}
        conclusion={summary?.conclusion}
        sessionDigest={{
          pid: session?.pid,
          device: `${deviceName} · ${devInfo.memory_used ?? '?'} / ${devInfo.memory_size ?? '?'} MB`,
          gpu_util: gpuTotal.GPU_utilization,
          sm_util: gpuTotal.SM_utilization,
          exec_delay: summary?.execution_delay,
          kernel_stistics: detail?.kernel_stistics,
          tensorCores_usage: detail?.tensorCores_usage,
          top_kernels: kernels
            .slice()
            .sort((a, b) => (b.total_delay_us || 0) - (a.total_delay_us || 0))
            .slice(0, 10)
            .map((k) => ({
              name: k.kernel_name,
              type: k.type_of_operation,
              total_us: k.total_delay_us,
              avg_us: k.avg_delay_us,
              runs: k.run_times,
              sm: k.SM_utilization,
              tc: k.use_tensorCore,
            })),
          memory_stistics: detail?.memory_stistics,
          memSummary: session?.data?.memSummary,
          memspikes_count: session?.data?.memspikes?.length,
          oom_count: session?.data?.memOom?.oomItem?.length,
          python_top_frames: (session?.data?.pythonHotFrames || []).slice(0, 15),
        }}
      />

      {/* ===== Section 1: 设备/利用率 + GPU/CPU 函数调用耗时 ===== */}
      <Row gutter={16} style={{ marginBottom: 16 }}>
        <Col xs={24} md={8} lg={7}>
          <Space direction="vertical" size={12} style={{ width: '100%' }}>
            <InfoCard
              title="设备信息"
              accent="#6366f1"
              items={[
                { label: '设备ID', value: deviceId },
                { label: '设备名', value: deviceName },
                { label: '显存大小', value: memSize ?? '-' },
                { label: 'CUDA 核心', value: cudaCores },
              ]}
            />
            <InfoCard
              title="利用率信息"
              accent="#10b981"
              items={[
                { label: 'GPU利用率', value: fmtPctNum(gpuTotal.GPU_utilization, 2) },
                { label: 'SM利用率', value: fmtPctNum(gpuTotal.SM_utilization, 2) },
                { label: '每SM活跃blocks', value: fmtNum(gpuTotal.active_blocks_per_SM) },
                { label: '每SM活跃warps', value: fmtNum(gpuTotal.active_warps_per_SM) },
              ]}
            />
          </Space>
        </Col>
        <Col xs={24} md={16} lg={17}>
          <ExecDelayBarChart data={summary?.execution_delay} />
        </Col>
      </Row>

      {/* ===== Section 2: Kernel 分析 三环形图 ===== */}
      <Card
        size="small"
        style={{ marginBottom: 16 }}
        title={<span><span style={{ color: '#10b981', marginRight: 6 }}>▧</span>Kernel 分析</span>}
      >
        <Row gutter={16}>
          <Col xs={24} md={8}>
            <DonutCard
              title="GPU内核函数调用时间占比"
              slices={buildKernelCategorySlices(detail?.kernel_stistics, summary?.execution_delay, kernels)}
            />
          </Col>
          <Col xs={24} md={8}>
            <DonutCard
              title="Tensor Cores 使用时间占比"
              slices={buildTensorCoreSlices(detail?.tensorCores_usage)}
            />
          </Col>
          <Col xs={24} md={8}>
            <DonutCard
              title="Kernel 函数调用时间占比"
              slices={buildTopKernelSlices(kernels)}
              longLabel
            />
          </Col>
        </Row>
      </Card>

      {/* ===== Section 3.5: Python 调用栈火焰图 ===== */}
      <PythonStackCard tree={session?.data?.stackInfo as FlameNode | undefined} hot={session?.data?.pythonHotFrames} />

      {/* ===== Section 4: Top 内核函数表 ===== */}
      <TopKernelTable kernels={kernels} />

      {/* ===== Section 5: GPU 显存使用情况（Reserved / Allocated 时间序列） ===== */}
      <MemoryUsageCard memStat={detail?.memory_stistics} />

      {/* ===== Section 6: CUDA 显存快照（内联 MemoryViz） ===== */}
      <MemorySnapshotCard
        analysisId={analysisId}
        pid={session?.pid}
        mmSnapUrl={session?.data?.mmSnapUrl}
        memSummary={session?.data?.memSummary}
        memspikes={session?.data?.memspikes}
        memOom={session?.data?.memOom}
      />
    </>
  );
};

// —— 左侧信息卡 —— //
const InfoCard: React.FC<{
  title: string;
  accent: string;
  items: Array<{ label: string; value: React.ReactNode }>;
}> = ({ title, accent, items }) => (
  <Card size="small" bordered style={{ borderRadius: 10 }}>
    <div style={{ borderLeft: `3px solid ${accent}`, paddingLeft: 8, fontSize: 14, fontWeight: 600, marginBottom: 14, color: '#111827' }}>
      {title}
    </div>
    <Row gutter={[16, 16]}>
      {items.map((it, i) => (
        <Col span={12} key={i}>
          <div style={{ color: '#6b7280', fontSize: 12, marginBottom: 4 }}>{it.label}</div>
          <div style={{ fontSize: 20, fontWeight: 500, color: '#111827', wordBreak: 'break-all' }}>{it.value}</div>
        </Col>
      ))}
    </Row>
  </Card>
);

// —— 横向条形图：GPU/CPU 函数调用耗时 —— //
const ExecDelayBarChart: React.FC<{ data?: ExecDelay }> = ({ data }) => {
  const [order, setOrder] = useState<'desc' | 'asc'>('desc');
  const items = useMemo(() => {
    if (!data) return [];
    const arr = Object.entries(data)
      .filter(([k, v]) => k !== 'total' && typeof v === 'number' && (v as number) > 0)
      .map(([k, v]) => ({ key: k, label: DELAY_LABEL[k] || k, value: v as number }));
    arr.sort((a, b) => (order === 'desc' ? b.value - a.value : a.value - b.value));
    return arr;
  }, [data, order]);
  const total = data?.total ?? 0;
  const maxV = items.length ? Math.max(...items.map(i => i.value)) : 0;
  const scaleMax = Math.max(maxV, total) * 1.05;

  if (!items.length) {
    return <Card size="small"><Empty description="无 execution_delay 数据" /></Card>;
  }

  const option = {
    grid: { left: 130, right: 90, top: 20, bottom: 30 },
    xAxis: {
      type: 'value',
      max: scaleMax,
      axisLine: { show: false }, splitLine: { show: false }, axisTick: { show: false }, axisLabel: { show: false },
    },
    yAxis: {
      type: 'category',
      data: items.map((it, i) => `${i + 1}  ${it.label}`).reverse(),
      axisLine: { show: false }, axisTick: { show: false },
      axisLabel: { color: '#374151', fontSize: 13, fontFamily: 'ui-monospace, Menlo, monospace' },
    },
    series: [
      {
        type: 'bar',
        data: items.slice().reverse().map((it, i) => ({
          value: it.value,
          itemStyle: { color: DELAY_COLORS[items.length - 1 - i] || '#6366f1', borderRadius: [0, 4, 4, 0] },
        })),
        barMaxWidth: 22,
        label: {
          show: true, position: 'right',
          formatter: (p: any) => fmtUs(p.value),
          color: '#111827', fontWeight: 600, fontSize: 13,
        },
        showBackground: true,
        backgroundStyle: { color: '#f3f4f6', borderRadius: 4 },
        markLine: total > 0 ? {
          symbol: 'none',
          silent: true,
          lineStyle: { color: '#ef4444', type: 'dashed', width: 1 },
          label: {
            formatter: `total ${fmtUs(total)}`,
            color: '#ef4444', fontSize: 11, position: 'end',
          },
          data: [{ xAxis: total }],
        } : undefined,
      },
    ],
    tooltip: { trigger: 'axis', axisPointer: { type: 'shadow' }, formatter: (ps: any) => `${ps[0].name}<br/>${fmtUs(ps[0].value)}` },
  };

  return (
    <Card size="small" style={{ borderRadius: 10 }}>
      <div style={{ display: 'flex', alignItems: 'center', marginBottom: 8 }}>
        <div style={{ borderLeft: '3px solid #6366f1', paddingLeft: 8, fontSize: 14, fontWeight: 600, color: '#111827', flex: 1 }}>
          GPU/CPU 函数调用耗时
        </div>
        <span style={{ color: '#6b7280', fontSize: 12, marginRight: 12 }}>total <b style={{ color: '#111827' }}>{fmtUs(total)}</b></span>
        <Space size={4}>
          <Button size="small" type={order === 'desc' ? 'link' : 'text'} onClick={() => setOrder('desc')}
            style={{ color: order === 'desc' ? '#6366f1' : '#6b7280', fontWeight: order === 'desc' ? 600 : 400 }}>
            从高到低
          </Button>
          <span style={{ color: '#d1d5db' }}>|</span>
          <Button size="small" type={order === 'asc' ? 'link' : 'text'} onClick={() => setOrder('asc')}
            style={{ color: order === 'asc' ? '#6366f1' : '#6b7280', fontWeight: order === 'asc' ? 600 : 400 }}>
            从低到高
          </Button>
        </Space>
      </div>
      <Alert
        type="warning" showIcon
        style={{ marginBottom: 8, fontSize: 12 }}
        message={
          <span style={{ fontSize: 12 }}>
            <b>total</b> 记录的是函数调用从开始到结束的物理时间，由于 CPU 和 GPU 并行执行，各类别耗时存在时间重叠，累计之和可能超过 total，属正常现象
          </span>
        }
      />
      <ReactECharts option={option} style={{ height: Math.max(220, items.length * 44) }} notMerge lazyUpdate />
    </Card>
  );
};

// —— GPU 显存使用情况 —— //
// Upstream memory_stistics is emitted by CF/pyki as three parallel arrays
// (Allocated / Reserved / Time). Values are in bytes; Time is in milliseconds
// despite the older "时间 (s)" label. Sample counts are often low (single
// digits) and the last sample tends to be (0, 0) — the teardown frame
// captured after the workload has released its tensors. Both artefacts
// mislead viewers, so we normalise them here before charting.
const MemoryUsageCard: React.FC<{
  memStat?: { Allocated?: number[]; Reserved?: number[]; Time?: number[] };
}> = ({ memStat }) => {
  const rawAlloc = memStat?.Allocated || [];
  const rawReserved = memStat?.Reserved || [];
  const rawTime = memStat?.Time || [];

  // Align to the shortest of the three arrays — defensive against upstream
  // producing mismatched lengths.
  const n = Math.min(rawAlloc.length, rawReserved.length, rawTime.length);

  // Drop trailing samples where both Allocated and Reserved are zero: those
  // are the profiler-teardown artefact and only serve to draw a cliff down
  // to zero on the chart. Keep intermediate zeros in case a real free
  // legitimately returned everything.
  let end = n;
  while (end > 0 && (rawAlloc[end - 1] || 0) === 0 && (rawReserved[end - 1] || 0) === 0) {
    end--;
  }
  const alloc = rawAlloc.slice(0, end);
  const reserved = rawReserved.slice(0, end);
  const timeMs = rawTime.slice(0, end);
  const droppedTail = n - end;

  const hasData = timeMs.length > 0 && (alloc.length > 0 || reserved.length > 0);
  if (!hasData) {
    return null;
  }

  const toMB = (b: number) => b / (1024 * 1024);
  // Convert ms to seconds for the axis; keep two decimals so short
  // (<1s) profiles still read as time rather than "0".
  const timeSec = timeMs.map((t) => (t / 1000).toFixed(2));

  const lowSamples = timeMs.length < 5;

  const option = {
    tooltip: {
      trigger: 'axis',
      formatter: (params: any[]) => {
        const t = params?.[0]?.axisValue ?? '';
        const rows = params
          .map((p) => `${p.marker}${p.seriesName}: <b>${toMB(p.value).toFixed(1)} MB</b>`)
          .join('<br/>');
        // Cached-free = Reserved - Allocated: memory PyTorch is holding
        // in its caching allocator but not currently handing to tensors.
        // Not the same as fragmentation; fragmentation is unsatisfiable
        // free space inside a segment, which needs segment/block layout
        // to compute.
        const idx = params?.[0]?.dataIndex ?? -1;
        const r = idx >= 0 ? reserved[idx] || 0 : 0;
        const a = idx >= 0 ? alloc[idx] || 0 : 0;
        const gap = Math.max(0, r - a);
        return `t = ${t} s<br/>${rows}<br/><span style="color:#9ca3af">cached-free: ${toMB(gap).toFixed(1)} MB</span>`;
      },
    },
    legend: { data: ['Reserved', 'Allocated'], top: 4, right: 8 },
    grid: { left: 60, right: 30, top: 40, bottom: 40 },
    xAxis: {
      type: 'category',
      name: '时间 (s)',
      nameLocation: 'middle',
      nameGap: 26,
      data: timeSec,
      boundaryGap: false,
    },
    yAxis: {
      type: 'value',
      name: '显存 (MB)',
      axisLabel: { formatter: (v: number) => `${toMB(v).toFixed(0)}` },
    },
    series: [
      {
        name: 'Reserved',
        type: 'line',
        smooth: true,
        showSymbol: reserved.length <= 20,
        areaStyle: { opacity: 0.15 },
        lineStyle: { width: 2 },
        color: '#f97316',
        data: reserved,
      },
      {
        name: 'Allocated',
        type: 'line',
        smooth: true,
        showSymbol: alloc.length <= 20,
        areaStyle: { opacity: 0.15 },
        lineStyle: { width: 2 },
        color: '#3b82f6',
        data: alloc,
      },
    ],
  };

  return (
    <Card
      size="small"
      style={{ marginBottom: 16 }}
      title={
        <span>
          <span style={{ color: '#8b5cf6', marginRight: 6 }}>▧</span>
          GPU 显存使用情况
          <span style={{ color: '#9ca3af', fontSize: 12, marginLeft: 10 }}>
            · 采样点 {timeMs.length}
            {droppedTail > 0 ? ` · 已丢弃 ${droppedTail} 个收尾归零点` : ''}
            · Reserved-Allocated = 缓存池预留冗余（非碎片）
          </span>
        </span>
      }
    >
      {lowSamples && (
        <div
          style={{
            marginBottom: 8,
            padding: '6px 10px',
            background: '#fff7ed',
            border: '1px solid #fed7aa',
            color: '#9a3412',
            fontSize: 12,
            borderRadius: 4,
          }}
        >
          采样点仅 {timeMs.length} 个，曲线仅供粗略参考；真实碎片请查看下方 CUDA 显存快照。
        </div>
      )}
      <ReactECharts option={option} style={{ height: 300 }} notMerge lazyUpdate />
    </Card>
  );
};

// —— CUDA 显存快照（内联 MemoryViz iframe，取代原独立 Tab） —— //
const MemorySnapshotCard: React.FC<{
  analysisId: string;
  pid?: string;
  mmSnapUrl?: string;
  memSummary?: string;
  memspikes?: any[];
  memOom?: { oomItem?: any[] };
}> = ({ analysisId, pid, mmSnapUrl, memSummary, memspikes, memOom }) => {
  if (!mmSnapUrl) {
    return null;
  }
  const embedUrl = withBase(`/ai_observable/result?analysisId=${encodeURIComponent(analysisId)}${
    pid ? `&pid=${encodeURIComponent(pid)}` : ''
  }&embed=memviz`);

  return (
    <Card
      size="small"
      style={{ marginBottom: 16 }}
      title={
        <span>
          <span style={{ color: '#8b5cf6', marginRight: 6 }}>▧</span>
          CUDA 显存快照
          <span style={{ color: '#9ca3af', fontSize: 12, marginLeft: 10 }}>
            · 显存尖峰 {memspikes?.length ?? 0} 条 · OOM {memOom?.oomItem?.length ?? 0} 条
          </span>
        </span>
      }
    >
      <Space direction="vertical" size={12} style={{ width: '100%' }}>
        <Alert
          type="info"
          showIcon
          message={
            <span>
              {memSummary || 'CUDA memory snapshot'}
              {pid ? <> · PID <code>{pid}</code></> : null}
            </span>
          }
        />
        <Space>
          <Button type="primary" href={mmSnapUrl} target="_blank" rel="noreferrer" download>
            下载 snapshot (pickle.gz)
          </Button>
          <Button href={embedUrl} target="_blank" rel="noreferrer">
            在完整 MemoryViz 中打开
          </Button>
        </Space>
        <iframe
          src={embedUrl}
          style={{ width: '100%', height: 720, border: '1px solid #e8eaed', borderRadius: 6, display: 'block' }}
          title="CUDA Memory Snapshot"
          loading="lazy"
        />
      </Space>
    </Card>
  );
};

// —— Python 调用栈火焰图（SVG，无第三方依赖） —— //
const FLAME_ROW_H = 18;
const FLAME_MIN_W = 2;

const flameColor = (name: string, depth: number): string => {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffff;
  const hue = 15 + (h % 45);          // 橙红系
  const sat = 65 + ((h >> 4) % 20);
  const light = 55 - Math.min(depth, 6) * 3;
  return `hsl(${hue} ${sat}% ${light}%)`;
};

type FlameCell = { x: number; y: number; w: number; depth: number; node: FlameNode };

const flattenFlame = (root: FlameNode, totalW: number): FlameCell[] => {
  const cells: FlameCell[] = [];
  const walk = (n: FlameNode, x: number, w: number, depth: number) => {
    if (w < FLAME_MIN_W) return;
    cells.push({ x, y: depth * FLAME_ROW_H, w, depth, node: n });
    const parentValue = n.value || 1;
    let cx = x;
    for (const c of n.children || []) {
      const cw = (c.value / parentValue) * w;
      walk(c, cx, cw, depth + 1);
      cx += cw;
    }
  };
  walk(root, 0, totalW, 0);
  return cells;
};

const PythonStackCard: React.FC<{
  tree?: FlameNode;
  hot?: Array<{ name: string; self_us: number }>;
}> = ({ tree, hot }) => {
  const [zoom, setZoom] = useState<FlameNode | null>(null);
  const [query, setQuery] = useState('');
  const totalW = 1100;

  const hasTree = !!(tree && tree.children && tree.children.length > 0 && (tree.value || 0) > 0);
  const active = zoom || tree || null;
  const cells = useMemo(() => (active && hasTree ? flattenFlame(active, totalW) : []), [active, hasTree]);
  const height = cells.length ? Math.max(...cells.map((c) => c.y)) + FLAME_ROW_H + 4 : 60;

  if (!hasTree) {
    return (
      <Card
        size="small"
        style={{ marginBottom: 16 }}
        title={<span><span style={{ color: '#ef4444', marginRight: 6 }}>🔥</span>Python 调用栈火焰图</span>}
      >
        <Empty
          description={
            <span style={{ color: '#6b7280' }}>
              无 Python 栈数据 — 采集时勾选「python 调用栈」再试
            </span>
          }
          style={{ padding: '30px 0' }}
        />
      </Card>
    );
  }

  const q = query.trim().toLowerCase();

  return (
    <Card
      size="small"
      style={{ marginBottom: 16 }}
      title={
        <span>
          <span style={{ color: '#ef4444', marginRight: 6 }}>🔥</span>
          Python 调用栈火焰图
          <span style={{ color: '#9ca3af', fontSize: 12, marginLeft: 10 }}>
            · 总耗时 {fmtUs((tree?.value || 0))} · 点击栈帧下钻，双击回到顶层
          </span>
        </span>
      }
      extra={
        <Space>
          <Input.Search
            allowClear
            size="small"
            placeholder="搜索函数名"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            style={{ width: 200 }}
          />
          {zoom && (
            <Button size="small" onClick={() => setZoom(null)}>
              返回顶层
            </Button>
          )}
        </Space>
      }
    >
      <div style={{ width: '100%', overflowX: 'auto', background: '#0f172a', borderRadius: 6, padding: 8 }}>
        <svg
          width={totalW}
          height={height}
          onDoubleClick={() => setZoom(null)}
          style={{ display: 'block', fontFamily: 'monospace' }}
        >
          {cells.map((c, i) => {
            const highlight = q && c.node.name.toLowerCase().includes(q);
            const dim = q && !highlight;
            return (
              <g
                key={i}
                onClick={(e) => {
                  e.stopPropagation();
                  setZoom(c.node);
                }}
                style={{ cursor: 'pointer' }}
              >
                <rect
                  x={c.x}
                  y={c.y}
                  width={Math.max(c.w - 1, 1)}
                  height={FLAME_ROW_H - 1}
                  fill={flameColor(c.node.name, c.depth)}
                  opacity={dim ? 0.25 : 1}
                  stroke={highlight ? '#fde68a' : 'none'}
                  strokeWidth={highlight ? 1.5 : 0}
                >
                  <title>
                    {c.node.name}
                    {'\n'}耗时 {fmtUs(c.node.value)}
                  </title>
                </rect>
                {c.w > 40 && (
                  <text
                    x={c.x + 4}
                    y={c.y + 13}
                    fill="#f8fafc"
                    fontSize={11}
                    pointerEvents="none"
                    style={{ userSelect: 'none' }}
                  >
                    {c.node.name.length * 7 > c.w
                      ? c.node.name.slice(0, Math.max(0, Math.floor(c.w / 7) - 1)) + '…'
                      : c.node.name}
                  </text>
                )}
              </g>
            );
          })}
        </svg>
      </div>

      {hot && hot.length > 0 && (
        <div style={{ marginTop: 12 }}>
          <div style={{ fontSize: 12, color: '#6b7280', marginBottom: 6 }}>Top Python 热点函数（累计 self）：</div>
          <Space size={[6, 6]} wrap>
            {hot.slice(0, 15).map((h) => (
              <Tag key={h.name} color="orange" style={{ margin: 0 }}>
                {h.name}
                <span style={{ marginLeft: 6, color: '#78350f' }}>{fmtUs(h.self_us)}</span>
              </Tag>
            ))}
          </Space>
        </div>
      )}
    </Card>
  );
};

// —— 环形图 —— //
type Slice = { name: string; value: number; color?: string; unit?: string };
const DonutCard: React.FC<{ title: string; slices: Slice[]; longLabel?: boolean }> = ({ title, slices, longLabel }) => {
  const total = slices.reduce((s, x) => s + x.value, 0);
  if (!total) {
    return (
      <Card size="small" style={{ borderRadius: 10, height: '100%' }} title={<span style={{ fontSize: 13 }}>{title}</span>}>
        <Empty description="无数据" style={{ padding: '30px 0' }} />
      </Card>
    );
  }
  const resolvedColors = slices.map((s, i) => s.color || DELAY_COLORS[i % DELAY_COLORS.length]);
  const seenNames = new Map<string, number>();
  const option = {
    tooltip: { trigger: 'item', formatter: (p: any) => `${p.name}<br/>${fmtUs(p.value)} · ${p.percent}%` },
    series: [{
      type: 'pie', radius: ['55%', '80%'], center: ['50%', '50%'],
      avoidLabelOverlap: false, label: { show: false }, labelLine: { show: false },
      data: slices.map((s, i) => {
        // ECharts 会按 name 归并同名 slice 的颜色索引，因此对重名再加零宽后缀
        // 保证 name 唯一但 tooltip 显示仍然是原文（tooltip 里我们主动展示 p.name）。
        const seen = seenNames.get(s.name) || 0;
        seenNames.set(s.name, seen + 1);
        const uniqueName = seen === 0 ? s.name : s.name + '​'.repeat(seen);
        return {
          name: uniqueName,
          value: s.value,
          itemStyle: { color: resolvedColors[i] },
        };
      }),
    }],
  };
  return (
    <Card size="small" style={{ borderRadius: 10, height: '100%' }}
      title={<span style={{ fontSize: 13, fontWeight: 600, color: '#111827' }}>{title}</span>}
    >
      <Row align="middle" gutter={4}>
        <Col span={longLabel ? 8 : 10}>
          <ReactECharts option={option} style={{ height: 200 }} notMerge lazyUpdate />
        </Col>
        <Col span={longLabel ? 16 : 14}>
          <div style={{ fontSize: 12 }}>
            {slices.map((s, i) => {
              const pct = (s.value / total * 100).toFixed(1);
              return (
                <div key={i} style={{ display: 'flex', alignItems: 'center', margin: '5px 0' }}>
                  <span style={{ display: 'inline-block', width: 10, height: 10, background: resolvedColors[i], borderRadius: 2, marginRight: 8, flexShrink: 0 }} />
                  <span style={{ color: '#6b7280', flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={s.name}>{s.name}</span>
                  <span style={{ color: '#111827', fontWeight: 500, marginLeft: 8, whiteSpace: 'nowrap' }}>{fmtUs(s.value)}</span>
                  <span style={{ color: '#9ca3af', marginLeft: 10, minWidth: 46, textAlign: 'right' }}>{pct}%</span>
                </div>
              );
            })}
          </div>
        </Col>
      </Row>
    </Card>
  );
};

// —— 环形图数据构造 —— //
function buildKernelCategorySlices(
  ks?: { Computation?: number; Memory?: number; Communication?: number },
  ed?: ExecDelay,
  kernels?: KernelDetail[],
): Slice[] {
  const comp = ks?.Computation || 0;
  const mem = ks?.Memory || 0;
  const comm = ks?.Communication || 0;
  // GPU 空闲 = total - kernel 类耗时（近似）— 只作为参考
  const kernelSum = comp + mem + comm;
  const total = ed?.total || kernelSum;
  const idle = Math.max(0, total - kernelSum);
  const arr: Slice[] = [];
  if (comm > 0) arr.push({ name: 'GPU通信用时', value: comm, color: '#f97316' });
  if (comp > 0) arr.push({ name: 'GPU运算用时', value: comp, color: '#3b82f6' });
  if (idle > 0) arr.push({ name: 'GPU空闲时间', value: idle, color: '#d1d5db' });
  if (mem > 0)  arr.push({ name: 'GPU显存操作', value: mem,  color: '#a78bfa' });
  // 如果 kernel_stistics 全 0 但 kernels 有数据，回退用 kernels 累加
  if (!arr.length && kernels?.length) {
    let c = 0, m = 0;
    for (const k of kernels) {
      const cat = classifyKernel(k);
      if (cat === 'memory') m += k.total_delay_us || 0;
      else c += k.total_delay_us || 0;
    }
    if (c > 0) arr.push({ name: 'GPU运算用时', value: c, color: '#3b82f6' });
    if (m > 0) arr.push({ name: 'GPU显存操作', value: m, color: '#a78bfa' });
  }
  return arr;
}

function buildTensorCoreSlices(tc?: { service_time?: number; unused_time?: number }): Slice[] {
  const used = tc?.service_time || 0;
  const unused = tc?.unused_time || 0;
  const arr: Slice[] = [];
  if (unused > 0) arr.push({ name: '未使用时间', value: unused, color: '#d1d5db' });
  if (used > 0)   arr.push({ name: '使用时间',   value: used,   color: '#10b981' });
  return arr;
}

function buildTopKernelSlices(kernels: KernelDetail[]): Slice[] {
  if (!kernels.length) return [];
  const sorted = [...kernels].sort((a, b) => (b.total_delay_us || 0) - (a.total_delay_us || 0));
  const top = sorted.slice(0, 6);
  const rest = sorted.slice(6);
  const colors = ['#3b82f6', '#f97316', '#10b981', '#a78bfa', '#ef4444', '#14b8a6'];
  const shorten = (n: string) => {
    if (n.length <= 22) return n;
    return n.substring(0, 22) + '…';
  };
  const arr: Slice[] = top.map((k, i) => ({
    name: shorten(k.kernel_name || ''),
    value: k.total_delay_us || 0,
    color: colors[i],
  }));
  const restSum = rest.reduce((s, k) => s + (k.total_delay_us || 0), 0);
  if (restSum > 0) arr.push({ name: '其他', value: restSum, color: '#f59e0b' });
  return arr;
}

// —— Top 内核函数表 —— //
const RANK_COLORS = ['#3b82f6', '#10b981', '#f59e0b', '#eeeeee', '#eeeeee', '#eeeeee', '#eeeeee', '#eeeeee'];

const TopKernelTable: React.FC<{ kernels: KernelDetail[] }> = ({ kernels }) => {
  const [search, setSearch] = useState('');
  const [filter, setFilter] = useState<'all' | keyof typeof KERNEL_TYPE_TAG>('all');
  const [topN, setTopN] = useState(20);

  const totalUs = kernels.reduce((s, k) => s + (k.total_delay_us || 0), 0);

  // 计算标准差（每 kernel 内部 max-min 近似作为 std 展示，或用 max-avg）
  const withStats = useMemo(() => kernels.map(k => ({
    ...k,
    _cat: classifyKernel(k),
    _std: k.max_delay_us != null && k.avg_delay_us != null ? Math.abs(k.max_delay_us - k.avg_delay_us) : 0,
    _pct: totalUs > 0 ? (k.total_delay_us || 0) / totalUs : 0,
  })), [kernels, totalUs]);

  const filtered = useMemo(() => {
    let arr = withStats;
    if (filter !== 'all') arr = arr.filter(k => k._cat === filter);
    if (search.trim()) {
      const q = search.trim().toLowerCase();
      arr = arr.filter(k => (k.kernel_name || '').toLowerCase().includes(q));
    }
    arr = [...arr].sort((a, b) => (b.total_delay_us || 0) - (a.total_delay_us || 0));
    return arr.slice(0, topN);
  }, [withStats, filter, search, topN]);

  const FILTER_CHIPS: Array<{ key: 'all' | keyof typeof KERNEL_TYPE_TAG; label: string }> = [
    { key: 'all',           label: '全部' },
    { key: 'computation',   label: '计算' },
    { key: 'communication', label: '通信' },
    { key: 'memory',        label: '内存' },
    { key: 'sync',          label: '同步' },
    { key: 'runtime',       label: '运行' },
  ];

  const cols: TableProps<any>['columns'] = [
    {
      title: '#', dataIndex: '_rank', width: 60, align: 'center',
      render: (_: any, __: any, i: number) => (
        <span
          style={{
            display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
            width: 26, height: 26, borderRadius: '50%',
            background: RANK_COLORS[i] || '#eeeeee',
            color: i < 3 ? '#fff' : '#6b7280',
            fontSize: 12, fontWeight: 600,
          }}
        >{i + 1}</span>
      ),
    },
    {
      title: '内核函数名', dataIndex: 'kernel_name', ellipsis: true,
      render: (v: string, r: any) => {
        const tag = KERNEL_TYPE_TAG[r._cat as keyof typeof KERNEL_TYPE_TAG];
        return (
          <Space size={8}>
            <Tag color={tag.color} style={{ borderRadius: 4, margin: 0 }}>{tag.label}</Tag>
            {r.use_tensorCore === '是' && <Tag color="green" style={{ borderRadius: 4, margin: 0 }}>TC</Tag>}
            <span style={{ fontFamily: 'ui-monospace, Menlo, monospace', fontSize: 12, color: '#374151' }} title={v}>{v}</span>
          </Space>
        );
      },
    },
    {
      title: '总时间', dataIndex: 'total_delay_us', width: 110, align: 'right',
      sorter: (a: any, b: any) => (a.total_delay_us || 0) - (b.total_delay_us || 0),
      defaultSortOrder: 'descend',
      render: (v: number) => <b style={{ color: '#2563eb' }}>{fmtUs(v)}</b>,
    },
    { title: '调用次数', dataIndex: 'run_times', width: 90, align: 'right' },
    {
      title: '平均时间', dataIndex: 'avg_delay_us', width: 110, align: 'right',
      render: (v: number) => <span style={{ color: '#ea580c' }}>{fmtUs(v)}</span>,
    },
    {
      title: '标准差', dataIndex: '_std', width: 100, align: 'right',
      render: (v: number) => <span style={{ color: '#6b7280' }}>{fmtUs(v)}</span>,
    },
    {
      title: '时间占比', dataIndex: '_pct', width: 200,
      render: (v: number) => (
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <div style={{ flex: 1, height: 6, background: '#e5e7eb', borderRadius: 3, overflow: 'hidden' }}>
            <div style={{ width: `${Math.min(100, v * 100)}%`, height: '100%', background: '#3b82f6' }} />
          </div>
          <span style={{ minWidth: 46, textAlign: 'right', fontVariantNumeric: 'tabular-nums', color: '#374151' }}>{(v * 100).toFixed(1)}%</span>
        </div>
      ),
    },
  ];

  return (
    <Card
      size="small" style={{ borderRadius: 10 }}
      title={
        <Space size={10}>
          <span style={{ color: '#3b82f6' }}>📊</span>
          <span style={{ fontSize: 14, fontWeight: 600 }}>Top 内核函数</span>
          <Tag color="default" style={{ borderRadius: 10 }}>{filtered.length} / {kernels.length}</Tag>
        </Space>
      }
      extra={
        <Space size={6} wrap>
          <Input.Search
            placeholder="搜索内核名..."
            allowClear
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            style={{ width: 200 }}
          />
          {FILTER_CHIPS.map(c => (
            <Button
              key={c.key} size="small"
              type={filter === c.key ? 'primary' : 'default'}
              onClick={() => setFilter(c.key)}
              style={{ borderRadius: 6 }}
            >{c.label}</Button>
          ))}
          <Select
            size="small"
            value={topN}
            onChange={setTopN}
            options={[10, 20, 50, 100].map(n => ({ value: n, label: `Top ${n}` }))}
            style={{ width: 92 }}
          />
        </Space>
      }
    >
      <p style={{ margin: '0 0 12px', color: '#6b7280', fontSize: 12 }}>点击行展开详情，支持搜索和类型过滤</p>
      {kernels.length ? (
        <Table
          rowKey={(r: any) => r.kernel_name}
          size="small"
          columns={cols}
          dataSource={filtered}
          pagination={false}
          expandable={{
            expandedRowRender: (r: any) => (
              <div style={{ padding: '8px 12px', background: '#f9fafb', borderRadius: 6, fontSize: 12 }}>
                <Row gutter={[24, 8]}>
                  <Col span={6}><b style={{ color: '#6b7280' }}>Grid:</b> <code>{r.grid || '-'}</code></Col>
                  <Col span={6}><b style={{ color: '#6b7280' }}>Block:</b> <code>{r.block || '-'}</code></Col>
                  <Col span={6}><b style={{ color: '#6b7280' }}>SM 利用率:</b> {fmtPctNum(r.SM_utilization, 2)}</Col>
                  <Col span={6}><b style={{ color: '#6b7280' }}>Tensor Core:</b> {r.use_tensorCore || '否'}</Col>
                  <Col span={6}><b style={{ color: '#6b7280' }}>最大耗时:</b> {fmtUs(r.max_delay_us)}</Col>
                  <Col span={6}><b style={{ color: '#6b7280' }}>最小耗时:</b> {fmtUs(r.min_delay_us)}</Col>
                  <Col span={6}><b style={{ color: '#6b7280' }}>Shared Mem:</b> {r.shared_memory_size ?? '-'}</Col>
                  <Col span={6}><b style={{ color: '#6b7280' }}>Registers/Thread:</b> {r.registers_per_thread ?? '-'}</Col>
                </Row>
              </div>
            ),
          }}
        />
      ) : (
        <Empty description="该会话未采到 GPU Kernel 数据（可能仅采集了 snapshot / 未启用 --gpu）" />
      )}
    </Card>
  );
};

export default ProfileTab;

// ============================================================================
// Profiling Agent 面板 —— 结论 + 两种 LLM 通路（自定义 / 阿里云官方）+ 上传数据
// ============================================================================

type AgentMode = 'aliyun' | 'openclaw' | 'local';

type AgentPayload = {
  mode: AgentMode;
  question: string;
  analysisId?: string;
  sessionDigest?: any;
  llm?: { baseUrl?: string; apiKey?: string; model?: string };
  uploadRef?: string;
};

type ProfilingAgentPanelProps = {
  analysisId?: string;
  conclusion?: string;
  sessionDigest: any;
};

const LS_RESULT_PREFIX = 'aiprof.aiResult:';

type ConclusionMode = 'aliyun' | 'openclaw' | 'local';
type Conclusion = {
    mode: ConclusionMode;
    label: string;
    content: string;
    model?: string;
    savedAt: number;
};

// localStorage 兼容层：旧格式 { label, text, savedAt } 会被包成一条 openclaw 结论
// （历史上「本地 OpenClaw 分析」是最常用的按钮，落到 openclaw 那栏最不会丢），
// 新格式直接存 { conclusions: Conclusion[] }。
const readCachedConclusions = (analysisId?: string): Conclusion[] => {
    if (!analysisId) return [];
    try {
        const raw = localStorage.getItem(LS_RESULT_PREFIX + analysisId);
        if (!raw) return [];
        const j = JSON.parse(raw);
        if (Array.isArray(j?.conclusions)) return j.conclusions as Conclusion[];
        if (j && (j.text || j.content)) {
            return [{
                mode: (j.mode as ConclusionMode) || 'openclaw',
                label: j.label || 'AI 分析结果',
                content: j.content || j.text || '',
                model: j.model,
                savedAt: j.savedAt || Date.now(),
            }];
        }
        return [];
    } catch {
        return [];
    }
};
const writeCachedConclusions = (analysisId: string | undefined, conclusions: Conclusion[]) => {
    if (!analysisId) return;
    try {
        localStorage.setItem(LS_RESULT_PREFIX + analysisId, JSON.stringify({ conclusions }));
    } catch {
        // quota exceeded — ignore
    }
};
const clearCachedConclusions = (analysisId?: string) => {
    if (!analysisId) return;
    try { localStorage.removeItem(LS_RESULT_PREFIX + analysisId); } catch {}
};

const modeLabel = (m: ConclusionMode) => (m === 'aliyun' ? '阿里云 Profiling Agent' : m === 'local' ? '本地 Agent' : '本地 OpenClaw');
const modeTagColor = (m: ConclusionMode) => (m === 'aliyun' ? 'blue' : m === 'local' ? 'green' : 'purple');

const ProfilingAgentPanel: React.FC<ProfilingAgentPanelProps> = ({ analysisId, conclusion, sessionDigest }) => {
  const [openclawOpen, setOpenclawOpen] = useState(false);
  const [localAgentOpen, setLocalAgentOpen] = useState(false);
  const [llmSettingsOpen, setLlmSettingsOpen] = useState(false);
  const [loading, setLoading] = useState<'agent' | 'openclaw' | 'local' | null>(null);
  const [conclusions, setConclusions] = useState<Conclusion[]>([]);
  const [errorByMode, setErrorByMode] = useState<Partial<Record<ConclusionMode, string>>>({});
  const [collapsedByMode, setCollapsedByMode] = useState<Record<ConclusionMode, boolean>>({ aliyun: false, openclaw: false, local: false });
  const [compareLayout, setCompareLayout] = useState<'side-by-side' | 'stacked'>('side-by-side');

  const latestByMode = useMemo(() => {
    const acc: Record<ConclusionMode, Conclusion | null> = { aliyun: null, openclaw: null, local: null };
    for (const c of conclusions) {
      const prev = acc[c.mode];
      if (!prev || (c.savedAt || 0) > (prev.savedAt || 0)) acc[c.mode] = c;
    }
    return acc;
  }, [conclusions]);
  const modesPresent = (['aliyun', 'openclaw', 'local'] as ConclusionMode[]).filter((m) => !!latestByMode[m]);
  const hasMulti = modesPresent.length >= 2;

  const upsertConclusion = (next: Conclusion) => {
    setConclusions((cur) => {
      const kept = cur.filter((c) => c.mode !== next.mode);
      const merged = [...kept, next];
      writeCachedConclusions(analysisId, merged);
      return merged;
    });
    setErrorByMode((e) => ({ ...e, [next.mode]: undefined }));
    setCollapsedByMode((c) => ({ ...c, [next.mode]: false }));
  };

  const removeConclusion = (mode: ConclusionMode) => {
    setConclusions((cur) => {
      const kept = cur.filter((c) => c.mode !== mode);
      if (kept.length === 0) clearCachedConclusions(analysisId);
      else writeCachedConclusions(analysisId, kept);
      return kept;
    });
    setErrorByMode((e) => ({ ...e, [mode]: undefined }));
    if (analysisId) {
      fetch(withBase(`/api/ai/analyze/result?analysisId=${encodeURIComponent(analysisId)}&mode=${encodeURIComponent(mode)}`), { method: 'DELETE' }).catch(() => {});
    }
  };

  useEffect(() => {
    if (!analysisId) {
      setConclusions([]);
      setErrorByMode({});
      return;
    }
    const cached = readCachedConclusions(analysisId);
    setConclusions(cached);
    setErrorByMode({});
    let cancelled = false;
    fetch(withBase(`/api/ai/analyze/result?analysisId=${encodeURIComponent(analysisId)}`))
      .then((r) => (r.ok ? r.json() : null))
      .then((j) => {
        if (cancelled) return;
        const data = j?.data;
        const remote: Conclusion[] = Array.isArray(data?.conclusions)
          ? data.conclusions
          : (data && (data.content || data.text)
              ? [{
                  mode: (data.mode as ConclusionMode) || 'openclaw',
                  label: data.label || 'AI 分析结果',
                  content: data.content || data.text || '',
                  model: data.model,
                  savedAt: data.savedAt || Date.now(),
                }]
              : []);
        if (!remote.length) return;
        // 按 mode 合并：同 mode 保留 savedAt 更大的一条
        setConclusions((local) => {
          const byMode = new Map<ConclusionMode, Conclusion>();
          for (const c of [...local, ...remote]) {
            const prev = byMode.get(c.mode);
            if (!prev || (c.savedAt || 0) > (prev.savedAt || 0)) byMode.set(c.mode, c);
          }
          const merged = Array.from(byMode.values());
          writeCachedConclusions(analysisId, merged);
          return merged;
        });
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [analysisId]);

  const [openclawForm] = Form.useForm();

  // An analysis runs for minutes; the gateway in front of the server kills
  // proxied requests idle for 60s. So POST only starts the job and the result
  // is collected by polling — a long-held POST would 504 mid-analysis and the
  // 504 HTML page would surface as the error message.
  const startAgent = async (payload: AgentPayload): Promise<void> => {
    const resp = await fetch(withBase('/api/ai/analyze'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    if (!resp.ok) {
      const t = await resp.text();
      let msg = t;
      if ((resp.headers.get('content-type') || '').includes('application/json')) {
        try { msg = JSON.parse(t).message || t; } catch { /* keep raw text */ }
      }
      throw new Error(msg || `HTTP ${resp.status}`);
    }
  };

  const pollAgentJob = async (
    analysisIdArg: string,
    mode: ConclusionMode,
  ): Promise<{ content: string; model?: string }> => {
    const url = withBase(`/api/ai/analyze/job?analysisId=${encodeURIComponent(analysisIdArg)}&mode=${mode}`);
    // A transient poll failure (gateway hiccup, page suspended) must not kill a
    // job that is still running server-side; only a run of them is fatal.
    let consecutiveFailures = 0;
    for (;;) {
      await new Promise((r) => setTimeout(r, 3000));
      let job: any = null;
      try {
        const resp = await fetch(url);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        job = (await resp.json())?.data;
        consecutiveFailures = 0;
      } catch (e: any) {
        if (++consecutiveFailures >= 10) throw new Error(`轮询分析进度失败：${e.message || e}`);
        continue;
      }
      // A restarted server loses its in-memory jobs; the conclusion, if the
      // analysis had finished, is on disk.
      if (!job) throw new Error('分析任务已丢失（服务端可能已重启），请重试。');
      if (job.status === 'succeeded') return { content: job.content, model: job.model };
      if (job.status === 'failed') throw new Error(job.message || '分析失败');
    }
  };

  const runAgent = async (
    mode: 'openclaw' | 'local',
    payload: AgentPayload,
  ): Promise<{ content: string; model?: string }> => {
    await startAgent(payload);
    return pollAgentJob(payload.analysisId!, mode);
  };

  const runOpenclaw = async (values: { question?: string; model?: string }) => {
    if (!analysisId) { message.warning('缺少 analysisId'); return; }
    setLoading('openclaw');
    setErrorByMode((e) => ({ ...e, openclaw: undefined }));
    try {
      const { content, model: resolvedModel } = await runAgent('openclaw', {
        mode: 'openclaw',
        question: values.question || '请对以下 GPU profiling 数据进行深入分析并给出优化建议。',
        analysisId,
        sessionDigest,
        llm: values.model ? { model: values.model } : undefined,
      });
      const label = `本地 OpenClaw 分析结果${resolvedModel ? ` · ${resolvedModel}` : ''}`;
      upsertConclusion({ mode: 'openclaw', label, content, model: resolvedModel, savedAt: Date.now() });
      setOpenclawOpen(false);
    } catch (e: any) {
      message.error(`OpenClaw 调用失败：${e.message || e}`);
      setErrorByMode((err) => ({
        ...err,
        openclaw: `OpenClaw 调用失败：${e.message || e}\n\n提示：若因缺 API Key 失败，请在弹窗内展开「⚙ LLM 设置」保存 Key；如未安装 OpenClaw，可在弹窗内点「一键安装 OpenClaw」。`,
      }));
    } finally {
      setLoading(null);
    }
  };

  const runLocalAgent = async (values: { question?: string; model?: string }) => {
    if (!analysisId) { message.warning('缺少 analysisId'); return; }
    setLoading('local');
    setErrorByMode((e) => ({ ...e, local: undefined }));
    try {
      const { content, model: resolvedModel } = await runAgent('local', {
        mode: 'local',
        question: values.question || '请对以下 GPU profiling 数据进行深入分析并给出优化建议。',
        analysisId,
        llm: values.model ? { model: values.model } : undefined,
      });
      const label = `本地 Agent 分析结果${resolvedModel ? ` · ${resolvedModel}` : ''}`;
      upsertConclusion({ mode: 'local', label, content, model: resolvedModel, savedAt: Date.now() });
      setLocalAgentOpen(false);
    } catch (e: any) {
      message.error(`本地 Agent 调用失败：${e.message || e}`);
      setErrorByMode((err) => ({
        ...err,
        local: `本地 Agent 调用失败：${e.message || e}\n\n提示：若因缺 API Key 失败，请到「LLM 全局配置」页面保存 API Key。`,
      }));
    } finally {
      setLoading(null);
    }
  };

  const renderConclusionCard = (mode: ConclusionMode) => {
    const c = latestByMode[mode];
    const err = errorByMode[mode];
    const isLoading = (mode === 'aliyun' && loading === 'agent') || (mode === 'openclaw' && loading === 'openclaw') || (mode === 'local' && loading === 'local');
    if (!c && !err && !isLoading) return null;
    const collapsed = collapsedByMode[mode];
    const title = c ? c.label : `${modeLabel(mode)} · 分析中…`;
    return (
      <Card
        size="small"
        type="inner"
        title={
          <Space size={8}>
            <Tag color={modeTagColor(mode)} style={{ marginRight: 0 }}>{modeLabel(mode)}</Tag>
            <span style={{ fontSize: 13 }}>{title}</span>
            {c?.savedAt && !isLoading ? (
              <Typography.Text type="secondary" style={{ fontSize: 11, fontWeight: 400 }}>
                · 已缓存 {new Date(c.savedAt).toLocaleString()}
              </Typography.Text>
            ) : null}
          </Space>
        }
        extra={
          c && !isLoading ? (
            <Space size={4}>
              <Button
                size="small"
                type="text"
                icon={collapsed ? <DownOutlined /> : <UpOutlined />}
                onClick={() => setCollapsedByMode((m) => ({ ...m, [mode]: !m[mode] }))}
              >
                {collapsed ? '展开' : '收起'}
              </Button>
              <Button
                size="small"
                type="text"
                danger
                onClick={() => removeConclusion(mode)}
              >
                清除
              </Button>
            </Space>
          ) : null
        }
        style={{ background: '#fafcff', height: '100%' }}
      >
        {isLoading ? (
          <div style={{ textAlign: 'center', padding: 24 }}>
            <Spin tip={`${modeLabel(mode)} 正在分析…`} />
          </div>
        ) : err ? (
          <Alert type="error" showIcon message={`${modeLabel(mode)} 调用失败`} description={
            <div style={{ whiteSpace: 'pre-wrap', fontSize: 12 }}>
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{err}</ReactMarkdown>
            </div>
          } />
        ) : collapsed ? (
          <Typography.Text type="secondary" style={{ fontSize: 12 }}>
            内容已收起（约 {c!.content.length} 字符）。点右上角「展开」查看。
          </Typography.Text>
        ) : (
          <div className="aiprof-md-result" style={{ fontSize: 13, lineHeight: 1.7, position: 'relative' }}>
            <ReactMarkdown
              remarkPlugins={[remarkGfm]}
              components={{
                table: (props) => (
                  <table style={{ borderCollapse: 'collapse', margin: '8px 0', width: '100%', fontSize: 12 }} {...props} />
                ),
                th: (props) => (
                  <th style={{ border: '1px solid #d9d9d9', padding: '4px 8px', background: '#f0f5ff', textAlign: 'left' }} {...props} />
                ),
                td: (props) => (
                  <td style={{ border: '1px solid #d9d9d9', padding: '4px 8px', verticalAlign: 'top' }} {...props} />
                ),
                code: ({ inline, ...props }: any) =>
                  inline ? (
                    <code style={{ background: '#f5f5f5', padding: '0 4px', borderRadius: 3, fontSize: 12 }} {...props} />
                  ) : (
                    <code style={{ display: 'block', background: '#0b1021', color: '#d1d5db', padding: 12, borderRadius: 4, fontSize: 12, overflow: 'auto' }} {...props} />
                  ),
                h1: (props) => <h3 style={{ marginTop: 12, marginBottom: 8, fontSize: 16 }} {...props} />,
                h2: (props) => <h4 style={{ marginTop: 12, marginBottom: 6, fontSize: 14 }} {...props} />,
                h3: (props) => <h5 style={{ marginTop: 10, marginBottom: 4, fontSize: 13 }} {...props} />,
              }}
            >
              {c!.content}
            </ReactMarkdown>
            <div style={{ marginTop: 4 }}>
              <Typography.Text copyable={{ text: c!.content }} style={{ fontSize: 12 }} type="secondary">
                复制原文
              </Typography.Text>
            </div>
          </div>
        )}
      </Card>
    );
  };

  const hasAliyunSlot = !!(latestByMode.aliyun || errorByMode.aliyun || loading === 'agent');
  const hasOpenclawSlot = !!(latestByMode.openclaw || errorByMode.openclaw || loading === 'openclaw');
  const hasLocalSlot = !!(latestByMode.local || errorByMode.local || loading === 'local');
  const activeSlots: ConclusionMode[] = [
    ...(hasAliyunSlot ? ['aliyun' as ConclusionMode] : []),
    ...(hasOpenclawSlot ? ['openclaw' as ConclusionMode] : []),
    ...(hasLocalSlot ? ['local' as ConclusionMode] : []),
  ];
  const anySlot = activeSlots.length > 0;

  return (
    <Card
      size="small"
      style={{ marginBottom: 16 }}
      title={
        <span>
          <ExperimentOutlined style={{ color: '#1a73e8', marginRight: 6 }} />
          AI 分析结论
        </span>
      }
      extra={
        <Space wrap>
          <Button
            icon={<SettingOutlined />}
            onClick={() => setLlmSettingsOpen(true)}
            title="配置 Base URL / API Key / 模型，全局生效于本地 Agent 与 OpenClaw 分析"
          >
            LLM 全局配置
          </Button>
          <Button
            icon={<ExperimentOutlined />}
            loading={loading === 'openclaw'}
            onClick={() => setOpenclawOpen(true)}
          >
            本地 OpenClaw 分析
          </Button>
          <Button
            icon={<ExperimentOutlined />}
            loading={loading === 'local'}
            onClick={() => setLocalAgentOpen(true)}
            disabled={!analysisId}
          >
            本地 Agent 分析
          </Button>
          <Button
            icon={<SwapOutlined />}
            disabled={!hasMulti}
            onClick={() => setCompareLayout((v) => (v === 'side-by-side' ? 'stacked' : 'side-by-side'))}
            title={hasMulti ? '并排 / 堆叠切换' : '两个及以上分析结论都在时才能对比'}
          >
            对比{compareLayout === 'side-by-side' ? '（并排）' : '（堆叠）'}
          </Button>
        </Space>
      }
    >
      {conclusion ? (
        <Typography.Paragraph style={{ marginBottom: anySlot ? 12 : 0, whiteSpace: 'pre-wrap' }}>
          {conclusion}
        </Typography.Paragraph>
      ) : (
        <Typography.Text type="secondary">规则式引擎未生成结论。点击右上按钮让 AI 深入分析。</Typography.Text>
      )}

      <Alert
        type="info"
        showIcon
        style={{ marginTop: 12 }}
        message={
          <span>
            <b>本地 OpenClaw</b> 走本机 agentic 分析；
            <b>本地 Agent</b> 走本机指标提取 + LLM 多轮循环 refine 出结论。
            两种结论会分别保留、互不覆盖，两个及以上在时可点右上「对比 ⇄」在并排 / 堆叠视图切换。
            如需阿里云 Profiling Agent 云端诊断，请前往顶部「阿里云 AI Profiling Demo」标签页体验。
          </span>
        }
      />

      {anySlot && (
        <div style={{ marginTop: 12 }}>
          {activeSlots.length >= 2 && compareLayout === 'side-by-side' ? (
            <Row gutter={12}>
              {activeSlots.map((m) => (
                <Col key={m} span={Math.floor(24 / activeSlots.length)}>{renderConclusionCard(m)}</Col>
              ))}
            </Row>
          ) : (
            <Space direction="vertical" style={{ width: '100%' }} size={12}>
              {activeSlots.map((m) => <React.Fragment key={m}>{renderConclusionCard(m)}</React.Fragment>)}
            </Space>
          )}
        </div>
      )}

      {/* 本地 OpenClaw 弹窗 */}
      <OpenclawModal
        open={openclawOpen}
        onClose={() => setOpenclawOpen(false)}
        loading={loading === 'openclaw'}
        form={openclawForm}
        onSubmit={runOpenclaw}
      />

      {/* 本地 Agent 弹窗（轻量：question + model） */}
      <LocalAgentModal
        open={localAgentOpen}
        onClose={() => setLocalAgentOpen(false)}
        loading={loading === 'local'}
        onSubmit={runLocalAgent}
      />

      {/* LLM 全局配置弹窗 */}
      <LlmSettingsModal open={llmSettingsOpen} onClose={() => setLlmSettingsOpen(false)} />
    </Card>
  );
};

// LLM 全局配置弹窗：报告页入口，复用 OpenClaw 弹窗里的同一套内联表单。
const LlmSettingsModal: React.FC<{ open: boolean; onClose: () => void }> = ({ open, onClose }) => {
  const [llmCfg, setLlmCfg] = React.useState<LlmServerCfg | null>(null);

  React.useEffect(() => {
    if (!open) return;
    (async () => {
      try {
        const r = await fetch(withBase('/api/settings/llm'));
        const j = await r.json();
        if (j?.code === 'ok' && j.data) setLlmCfg(j.data);
      } catch { /* 弹窗内的表单会显示未配置态，不额外打扰用户 */ }
    })();
  }, [open]);

  return (
    <Modal
      open={open}
      onCancel={onClose}
      footer={null}
      width={720}
      destroyOnClose
      title={
        <Space>
          <SettingOutlined style={{ color: '#1a73e8' }} />
          LLM 全局配置
        </Space>
      }
    >
      <Alert
        type="info"
        showIcon
        style={{ marginBottom: 12 }}
        message="全局生效：本地 Agent 分析、本地 OpenClaw 分析都使用此处配置的 Base URL 和 API Key。"
        description={
          <span style={{ fontSize: 12 }}>
            🔒 Key 仅存于本机 <code>/var/lib/aiprof/state/llm-config.json</code>，不会上传到任何云端。
          </span>
        }
      />
      <LlmInlineSettings cfg={llmCfg} onSaved={(next) => setLlmCfg(next)} />
    </Modal>
  );
};

const LocalAgentModal: React.FC<{
  open: boolean;
  loading: boolean;
  onClose: () => void;
  onSubmit: (v: { question?: string; model?: string }) => void;
}> = ({ open, loading, onClose, onSubmit }) => {
  const [form] = Form.useForm<{ question?: string }>();
  const [llmCfg, setLlmCfg] = React.useState<LlmServerCfg | null>(null);
  const [llmOpen, setLlmOpen] = React.useState(false);

  React.useEffect(() => {
    if (!open) { setLlmOpen(false); return; }
    (async () => {
      try {
        const r = await fetch(withBase('/api/settings/llm'));
        if (r.ok) {
          const j = await r.json();
          if (j?.code === 'ok' && j.data) setLlmCfg(j.data);
        }
      } catch { /* ignore */ }
    })();
  }, [open]);

  return (
    <Modal
      title="本地 Agent 分析"
      open={open}
      onCancel={onClose}
      onOk={() => form.submit()}
      okText="开始分析"
      confirmLoading={loading}
      width={640}
      destroyOnClose
    >
      <Alert
        type="success"
        showIcon
        style={{ marginBottom: 12 }}
        message="本地 Agent 已就绪（内置轻量分析，多轮循环 refine 出结论）"
      />

      <div style={{
        border: '1px solid #e5e7eb',
        borderRadius: 6,
        padding: llmOpen ? 12 : '8px 12px',
        marginBottom: 12,
        background: llmOpen ? '#fafbff' : '#fafafa',
      }}>
        <div
          style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer' }}
          onClick={() => setLlmOpen((v) => !v)}
        >
          <SettingOutlined style={{ color: '#1a73e8' }} />
          <span style={{ fontWeight: 500 }}>LLM 设置</span>
          {llmCfg?.hasApiKey ? (
            <Tag color="green" style={{ marginLeft: 4 }}>
              已配置 · {llmCfg.provider} · <code style={{ fontSize: 11 }}>{llmCfg.apiKeyMasked}</code>
            </Tag>
          ) : llmCfg?.envHasKey ? (
            <Tag color="blue" style={{ marginLeft: 4 }}>使用环境变量 QWEN_API_KEY 兜底</Tag>
          ) : (
            <Tag color="orange" style={{ marginLeft: 4 }}>未配置 API Key，点此展开设置</Tag>
          )}
          <div style={{ flex: 1 }} />
          {llmOpen ? <UpOutlined /> : <DownOutlined />}
        </div>
        {llmOpen && (
          <div style={{ marginTop: 12 }}>
            <LlmInlineSettings cfg={llmCfg} onSaved={(next) => setLlmCfg(next)} />
          </div>
        )}
      </div>

      <Form form={form} layout="vertical" onFinish={(v) => onSubmit({ question: v.question })} initialValues={{}}>
        <Alert
          type="info"
          showIcon
          style={{ marginBottom: 12 }}
          message="仅分析本次报告数据"
          description="需要分析本机 chrome-tracing JSON 文件？请使用记录行的「阿里云 Demo 体验」按钮，下载 trace 后到 Demo 环境上传分析。"
        />
        <Form.Item label="问题描述（可选）" name="question">
          <Input.TextArea rows={3} placeholder="留空则做通用性分析。例如：请分析 top kernel 热点和 GPU 空转原因。" />
        </Form.Item>
      </Form>
    </Modal>
  );
};

// —— 本地 OpenClaw 弹窗 —— //
// 三态：未安装（一键安装 + 日志尾）/ 已安装（表单）/ 分析中。
// 状态来自 GET /api/ai/openclaw/status；安装走 POST /api/ai/openclaw/install，之后轮询。
type OpenclawStatus = {
  install: { status: 'idle' | 'running' | 'ok' | 'error'; startedAt: number | null; endedAt: number | null; tail: string[]; error: string };
  openclaw: { installed: boolean; qwenReady: boolean; binary: string; version?: string };
  defaults: { model: string; hasServerQwenKey: boolean };
};

type LlmServerCfg = {
  provider: string;
  baseUrl: string;
  apiKeyMasked: string;
  hasApiKey: boolean;
  model: string;
  updatedAt: number;
  envHasKey: boolean;
  storagePath?: string;
};

type ProviderKey = 'dashscope' | 'openai' | 'deepseek' | 'zhipu' | 'moonshot' | 'custom';
type ProviderMeta = { label: string; emoji: string; baseUrl: string; keyPlaceholder: string; keyHintUrl?: string; keyHintText: string; models: string[]; verified?: boolean };

const LLM_PROVIDERS: Record<ProviderKey, ProviderMeta> = {
  dashscope: { label: '阿里云 DashScope (Qwen)', emoji: '☁️', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', keyPlaceholder: 'sk-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx', keyHintUrl: 'https://dashscope.console.aliyun.com/apiKey', keyHintText: 'DashScope 控制台', models: ['qwen3.7-plus', 'qwen3.7-max', 'qwen3.7-flash', 'qwen3.6-plus', 'qwen3.5-plus', 'qwen3-coder-plus'], verified: true },
  openai:    { label: 'OpenAI',           emoji: '🟢', baseUrl: 'https://api.openai.com/v1', keyPlaceholder: 'sk-proj-...', keyHintUrl: 'https://platform.openai.com/api-keys', keyHintText: 'OpenAI Platform', models: ['gpt-4o', 'gpt-4o-mini', 'gpt-4-turbo', 'gpt-3.5-turbo'] },
  deepseek:  { label: 'DeepSeek',         emoji: '🐳', baseUrl: 'https://api.deepseek.com/v1', keyPlaceholder: 'sk-...', keyHintUrl: 'https://platform.deepseek.com/api_keys', keyHintText: 'DeepSeek 平台', models: ['deepseek-chat', 'deepseek-reasoner'] },
  zhipu:     { label: '智谱 GLM',         emoji: '🔮', baseUrl: 'https://open.bigmodel.cn/api/paas/v4', keyPlaceholder: 'xxxxxxxx.xxxxxxxx', keyHintUrl: 'https://bigmodel.cn/usercenter/apikeys', keyHintText: '智谱 AI 开放平台', models: ['glm-4-plus', 'glm-4', 'glm-4-air', 'glm-4-flash'] },
  moonshot:  { label: 'Moonshot 月之暗面', emoji: '🌙', baseUrl: 'https://api.moonshot.cn/v1', keyPlaceholder: 'sk-...', keyHintUrl: 'https://platform.moonshot.cn/console/api-keys', keyHintText: 'Moonshot 开放平台', models: ['moonshot-v1-8k', 'moonshot-v1-32k', 'moonshot-v1-128k'] },
  custom:    { label: '自定义端点',        emoji: '⚙️', baseUrl: '', keyPlaceholder: '任意 OpenAI 兼容端点的 API Key', keyHintText: '填入任何 OpenAI 兼容的 /v1 endpoint（如私有部署 vLLM / Ollama）', models: [] },
};

// 折叠式的 LLM 设置面板，直接在 OpenClaw 弹窗里保存到 /api/settings/llm
const LlmInlineSettings: React.FC<{
  cfg: LlmServerCfg | null;
  onSaved: (next: LlmServerCfg) => void;
}> = ({ cfg, onSaved }) => {
  const [form] = Form.useForm<{ provider: ProviderKey; baseUrl: string; apiKey: string; model: string }>();
  const [provider, setProvider] = React.useState<ProviderKey>('dashscope');
  const [showKey, setShowKey] = React.useState(false);
  const [saving, setSaving] = React.useState(false);
  const meta = LLM_PROVIDERS[provider];
  const modelOptions = React.useMemo(
    () => (meta.models.length ? meta.models : []).map((m) => ({ label: m, value: m })),
    [meta],
  );

  React.useEffect(() => {
    if (!cfg) return;
    const p = (LLM_PROVIDERS as any)[cfg.provider] ? (cfg.provider as ProviderKey) : 'dashscope';
    setProvider(p);
    form.setFieldsValue({
      provider: p,
      baseUrl: cfg.baseUrl || LLM_PROVIDERS[p].baseUrl,
      apiKey: '',
      model: cfg.model || LLM_PROVIDERS[p].models[0] || '',
    });
  }, [cfg, form]);

  const onProviderChange = (v: ProviderKey) => {
    setProvider(v);
    const m = LLM_PROVIDERS[v];
    form.setFieldsValue({
      provider: v,
      baseUrl: m.baseUrl,
      model: m.models[0] || form.getFieldValue('model') || '',
    });
  };

  const onSave = async () => {
    try {
      const values = await form.validateFields();
      setSaving(true);
      const body: any = {
        provider: values.provider,
        baseUrl: values.baseUrl.trim(),
        model: values.model.trim(),
      };
      if (values.apiKey && values.apiKey.trim()) body.apiKey = values.apiKey.trim();
      const r = await fetch(withBase('/api/settings/llm'), {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(body),
      });
      const j = await r.json();
      if (j?.code !== 'ok') throw new Error(j?.message || `HTTP ${r.status}`);
      form.setFieldsValue({ apiKey: '' });
      onSaved(j.data);
      message.success('LLM 配置已保存');
    } catch (e: any) {
      if (e?.errorFields) return;
      message.error(`保存失败：${e?.message || e}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <Form form={form} layout="vertical" size="small" style={{ marginTop: 4 }}>
      <Alert
        type="warning"
        showIcon
        style={{ marginBottom: 12 }}
        message="🔒 本地存储，不会上传到阿里云或任何云端"
        description={
          <Space direction="vertical" size={2} style={{ fontSize: 12 }}>
            <span>
              存储位置：<code style={{ userSelect: 'all' }}>{cfg?.storagePath || '/var/lib/aiprof/state/llm-config.json'}</code>
            </span>
            <span>安全提示：Key 以明文存于本机该文件（权限 600），请确保本机访问可控；接口回显时 Key 一律脱敏为 <code>sk-****abcd</code>。</span>
            <span>数据流：仅在你点击「开始分析」时，由本机 aiprof-server 直接向所选云服务厂商发起 HTTPS 调用；aiprof-server 不会把 Key 或分析数据外传到任何第三方。</span>
          </Space>
        }
      />
      <Form.Item label="云服务厂商" name="provider" rules={[{ required: true }]} style={{ marginBottom: 8 }}>
        <Select
          onChange={(v) => onProviderChange(v as ProviderKey)}
          options={Object.entries(LLM_PROVIDERS).map(([k, m]) => ({
            value: k,
            label: (
              <span>
                <span style={{ marginRight: 6 }}>{m.emoji}</span>
                {m.label}
                {m.verified && <Tag color="green" style={{ marginLeft: 8 }}>已验证</Tag>}
              </span>
            ),
          }))}
        />
      </Form.Item>

      <Form.Item label="Base URL" name="baseUrl" rules={[{ required: true, message: '请填 Base URL' }]} style={{ marginBottom: 8 }}>
        <Input placeholder={LLM_PROVIDERS[provider].baseUrl || 'https://your-endpoint/v1'} autoComplete="off" spellCheck={false} />
      </Form.Item>

      <Form.Item
        label="API Key"
        name="apiKey"
        style={{ marginBottom: 8 }}
        extra={
          <Space direction="vertical" size={2} style={{ marginTop: 2 }}>
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              {meta.keyHintUrl
                ? <>在 <a href={meta.keyHintUrl} target="_blank" rel="noreferrer">{meta.keyHintText}</a> 获取 API Key</>
                : meta.keyHintText}
            </Typography.Text>
            {cfg?.hasApiKey && (
              <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                当前已保存：<code>{cfg.apiKeyMasked}</code>。留空则保留旧值不改动。
              </Typography.Text>
            )}
          </Space>
        }
      >
        <Input.Password
          placeholder={cfg?.hasApiKey ? '留空 = 保留已保存的 Key；填入新值 = 覆盖' : meta.keyPlaceholder}
          autoComplete="off"
          spellCheck={false}
          visibilityToggle={{ visible: showKey, onVisibleChange: setShowKey }}
          iconRender={(v) => (v ? <EyeOutlined /> : <EyeInvisibleOutlined />)}
          {...({ 'data-lpignore': 'true' } as any)}
        />
      </Form.Item>

      <Form.Item
        label="模型（保存后作为分析默认模型；可选择或直接输入自定义模型名）"
        name="model"
        rules={[{ required: true, message: '请选择或填写模型名' }]}
        style={{ marginBottom: 8 }}
        extra={
          <Typography.Text type="secondary" style={{ fontSize: 12 }}>
            未列出的模型可直接输入名称。DashScope + Qwen provider 目前支持：<code>qwen3.7-plus</code>、<code>qwen3.7-max</code>、<code>qwen3.7-flash</code>、<code>qwen3.6-plus</code>、<code>qwen3.5-plus</code>、<code>qwen3-coder-plus</code>。切换到 OpenAI/DeepSeek/Moonshot/GLM 等厂商时用对应厂商的官方模型 ID。
          </Typography.Text>
        }
      >
        <AutoComplete
          options={modelOptions}
          placeholder="选择或输入模型名"
          filterOption={(input, option) => String(option?.value ?? '').toLowerCase().includes(input.toLowerCase())}
          allowClear
        >
          <Input autoComplete="off" spellCheck={false} />
        </AutoComplete>
      </Form.Item>

      <Button type="primary" size="small" icon={<SaveOutlined />} onClick={onSave} loading={saving}>
        保存 LLM 配置
      </Button>
    </Form>
  );
};

const OpenclawModal: React.FC<{
  open: boolean;
  loading: boolean;
  onClose: () => void;
  form: any;
  onSubmit: (v: { question?: string; model?: string }) => void;
}> = ({ open, loading, onClose, form, onSubmit }) => {
  const [status, setStatus] = React.useState<OpenclawStatus | null>(null);
  const [polling, setPolling] = React.useState(false);
  const [installing, setInstalling] = React.useState(false);
  const [llmCfg, setLlmCfg] = React.useState<LlmServerCfg | null>(null);
  const [llmOpen, setLlmOpen] = React.useState(false);

  const fetchStatus = React.useCallback(async () => {
    try {
      const r = await fetch(withBase('/api/ai/openclaw/status'));
      if (r.ok) setStatus(await r.json());
    } catch { /* ignore, UI shows stale */ }
    try {
      const r2 = await fetch(withBase('/api/settings/llm'));
      if (r2.ok) {
        const j = await r2.json();
        if (j?.code === 'ok' && j.data) setLlmCfg(j.data);
      }
    } catch { /* ignore */ }
  }, []);

  React.useEffect(() => {
    if (!open) { setPolling(false); setLlmOpen(false); return; }
    fetchStatus();
  }, [open, fetchStatus]);

  React.useEffect(() => {
    if (!open || !polling) return;
    const iv = setInterval(fetchStatus, 1500);
    return () => clearInterval(iv);
  }, [open, polling, fetchStatus]);

  React.useEffect(() => {
    const s = status?.install?.status;
    if (s === 'running') setPolling(true);
    else if (s === 'ok' || s === 'error') { setPolling(false); setInstalling(false); }
  }, [status?.install?.status]);

  const startInstall = async () => {
    setInstalling(true);
    setPolling(true);
    try {
      const r = await fetch(withBase('/api/ai/openclaw/install'), { method: 'POST' });
      if (!r.ok && r.status !== 409) {
        const j = await r.json().catch(() => ({}));
        message.error(`触发安装失败：${j.message || r.status}`);
        setInstalling(false);
        setPolling(false);
      }
    } catch (e: any) {
      message.error(`触发安装失败：${e.message || e}`);
      setInstalling(false);
      setPolling(false);
    }
  };

  const ready = status?.openclaw?.installed;
  const qwenReady = status?.openclaw?.qwenReady;
  const installState = status?.install?.status || 'idle';
  const tail = (status?.install?.tail || []).slice(-14).join('\n');
  const canSubmit = !!ready;

  return (
    <Modal
      title="本地 OpenClaw 分析当前报告"
      open={open}
      onCancel={onClose}
      onOk={() => form.submit()}
      okText="开始分析"
      okButtonProps={{ disabled: !canSubmit, loading }}
      confirmLoading={loading}
      width={680}
    >
      {!status ? (
        <div style={{ textAlign: 'center', padding: 24 }}><Spin tip="检测 OpenClaw 状态中…" /></div>
      ) : !ready ? (
        <>
          <Alert
            type="warning"
            showIcon
            style={{ marginBottom: 12 }}
            message="本机未检测到 OpenClaw"
            description={
              <span style={{ fontSize: 12 }}>
                将执行 <code>npm i -g --prefix &lt;server data&gt; openclaw</code> 及 <code>openclaw plugins install @openclaw/qwen-provider</code>，
                安装到服务端私有目录（不污染宿主 npm 全局）。约 1-2 分钟。
              </span>
            }
          />
          <Space style={{ marginBottom: 12 }}>
            <Button
              type="primary"
              loading={installing || installState === 'running'}
              disabled={installState === 'running'}
              onClick={startInstall}
            >
              一键安装 OpenClaw + Qwen provider
            </Button>
            <Button onClick={fetchStatus}>刷新状态</Button>
            {installState === 'error' && <Typography.Text type="danger">{status.install.error}</Typography.Text>}
          </Space>
          {tail && (
            <Typography.Paragraph style={{ fontSize: 11, background: '#0b1021', color: '#d1d5db', padding: 8, borderRadius: 4, whiteSpace: 'pre-wrap', maxHeight: 200, overflow: 'auto' }}>
              {tail}
            </Typography.Paragraph>
          )}
          <Typography.Text type="secondary" style={{ fontSize: 12 }}>
            也可手动执行：<code>npm i -g openclaw && openclaw plugins install @openclaw/qwen-provider</code>，然后重启 aiprof-server。
          </Typography.Text>
        </>
      ) : (
        <>
          <Alert
            type="success"
            showIcon
            style={{ marginBottom: 12 }}
            message={`OpenClaw 已就绪${status.openclaw.version ? ` · ${status.openclaw.version}` : ''}${qwenReady ? '（Qwen provider 可用）' : '（未检测到 Qwen provider）'}`}
          />

          <div style={{
            border: '1px solid #e5e7eb',
            borderRadius: 6,
            padding: llmOpen ? 12 : '8px 12px',
            marginBottom: 12,
            background: llmOpen ? '#fafbff' : '#fafafa',
          }}>
            <div
              style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer' }}
              onClick={() => setLlmOpen((v) => !v)}
            >
              <SettingOutlined style={{ color: '#1a73e8' }} />
              <span style={{ fontWeight: 500 }}>LLM 设置</span>
              {llmCfg?.hasApiKey ? (
                <Tag color="green" style={{ marginLeft: 4 }}>
                  已配置 · {llmCfg.provider} · <code style={{ fontSize: 11 }}>{llmCfg.apiKeyMasked}</code>
                </Tag>
              ) : (llmCfg?.envHasKey || status.defaults.hasServerQwenKey) ? (
                <Tag color="blue" style={{ marginLeft: 4 }}>使用环境变量 QWEN_API_KEY 兜底</Tag>
              ) : (
                <Tag color="orange" style={{ marginLeft: 4 }}>未配置 API Key，点此展开设置</Tag>
              )}
              <div style={{ flex: 1 }} />
              {llmOpen ? <UpOutlined /> : <DownOutlined />}
            </div>
            {llmOpen && (
              <div style={{ marginTop: 12 }}>
                <LlmInlineSettings
                  cfg={llmCfg}
                  onSaved={(next) => {
                    setLlmCfg(next);
                    // 同步默认模型到分析表单（用户没手改的话）
                    if (next.model) form.setFieldsValue({ model: next.model });
                  }}
                />
              </div>
            )}
          </div>

          <Form
            form={form}
            layout="vertical"
            onFinish={(values: any) => {
              onSubmit({
                question: values.question,
              });
            }}
          >
            <Alert
              type="info"
              showIcon
              style={{ marginBottom: 12 }}
              message="仅分析本次报告数据"
              description="需要分析本机 chrome-tracing JSON 文件？请使用记录行的「阿里云 Demo 体验」按钮，下载 trace 后到 Demo 环境上传分析。"
            />
            <Form.Item label="补充问题（可选）" name="question">
              <Input.TextArea rows={3} placeholder="留空则请 OpenClaw 自行给出通用瓶颈/优化建议。例如：SM 利用率为何这么低？给出具体的融合/graph capture 建议。" />
            </Form.Item>
          </Form>
        </>
      )}
    </Modal>
  );
};
