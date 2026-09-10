// SPDX-License-Identifier: Apache-2.0
import React, { useEffect, useMemo, useState } from 'react';
import {
  Card, Form, Input, Select, Button, InputNumber, Space, Row, Col, message, Tabs, Tooltip,
} from 'antd';
import { StartAIAnalysis, GetListRecord, ListProfilingClients, ListGpuProcs } from '../../../api';
import { AiRecordTable, AnalysisRecord } from '../components/AiRecordTable';

// The demo lives under a path prefix, not at the domain root — linking to the
// bare host lands on the community homepage instead of the analyser.
// Anchor of the "阿里云 GPU 性能与诊断" tab. Points at the AI Infra
// Observation entry of the Alibaba Cloud operating-system console —
// the productionised path users should adopt for ongoing use.
const ALIYUN_CONSOLE_URL = import.meta.env.VITE_ALIYUN_CONSOLE_URL
  || 'https://alinux.console.aliyun.com/sysom-ai/ai-infra-observation';

const MAX_ANALYSIS_TIME_MS = 60000;
const MIN_ANALYSIS_TIME_MS = 1000;

interface Props {
  channel: string;
}

export const CapturePage: React.FC<Props> = ({ channel }) => {
  const [form] = Form.useForm();
  const [records, setRecords] = useState<AnalysisRecord[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [pagination, setPagination] = useState({ current: 1, pageSize: 10 });
  const [mode, setMode] = useState<'duration' | 'iteration'>('duration');
  const [clients, setClients] = useState<Array<{ clientId: string; status: string; wsConnected: boolean; transport?: 'ws' | 'poll' }>>([]);
  const [autoLoading, setAutoLoading] = useState(false);
  const [activeTab, setActiveTab] = useState('records');

  const refreshClients = async () => {
    const resp: any = await ListProfilingClients();
    if (resp?.code === 'Success' && Array.isArray(resp.data)) {
      setClients(resp.data);
    }
  };

  useEffect(() => {
    refreshClients();
    const id = setInterval(refreshClients, 15_000);
    return () => clearInterval(id);
  }, []);

  const refresh = async () => {
    setLoading(true);
    const resp: any = await GetListRecord({
      current: pagination.current,
      pageSize: pagination.pageSize,
    });
    setLoading(false);
    if (resp.code === 'Success' && Array.isArray(resp.data)) {
      setTotal(resp.total ?? resp.data.length);
      setRecords(resp.data as AnalysisRecord[]);
    } else if (resp.code !== 'Success') {
      message.error(`加载记录失败：${resp.message ?? 'unknown'}`);
    }
  };

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, 10_000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pagination.current, pagination.pageSize]);

  const onFinish = async (values: any) => {
    if (mode === 'duration') {
      const t = Number(values.analysisTimeout);
      if (Number.isNaN(t) || t < MIN_ANALYSIS_TIME_MS || t > MAX_ANALYSIS_TIME_MS) {
        message.warning(`采集时长需在 ${MIN_ANALYSIS_TIME_MS}-${MAX_ANALYSIS_TIME_MS} ms 之间`);
        return;
      }
    }
    // 自动发现填入的 pids 形如 "12345 (python3), 67890 (torchrun)"，提交前把括号里的
    // 进程名剥掉，只保留纯 PID 列表；手动输入的（如 "12345,67890" 或 "12345 67890"）
    // 也能被同一套逻辑正常处理。
    const rawPids = (values.pids || '').toString();
    let cleanPids = rawPids
      .split(/[,\s]+/)
      .map((t: string) => t.replace(/\(.*?\)/g, '').trim())
      .filter(Boolean)
      .join(',');

    // 提交前拉一次当前活跃 GPU 进程，对齐用户输入的 PID —— 供应端 (run-test.sh
    // 之类的守护脚本) 常在 python 崩溃后重启新 PID，UI 里缓存的老 PID 一发下去
    // ptrace 就 "No such process"，然后 cuprof 静默失败 0 字节 trace。
    if (cleanPids) {
      try {
        const live: any = await ListGpuProcs(values.instance);
        if (live?.code === 'Success' && Array.isArray(live.data)) {
          const alivePids: string[] = live.data.map((p: any) => String(p.pid));
          const aliveSet = new Set(alivePids);
          const requested = cleanPids.split(',');
          const stillAlive = requested.filter((p: string) => aliveSet.has(p));
          const dead = requested.filter((p: string) => !aliveSet.has(p));
          if (dead.length) {
            if (stillAlive.length) {
              message.warning(`PID ${dead.join(',')} 已退出，仅采集 ${stillAlive.join(',')}`);
              cleanPids = stillAlive.join(',');
            } else if (alivePids.length) {
              message.warning(`原 PID ${dead.join(',')} 已退出，改用当前活跃 GPU 进程：${alivePids.join(',')}`);
              cleanPids = alivePids.join(',');
              form.setFieldsValue({
                pids: live.data.map((p: any) => `${p.pid} (${p.name || 'unknown'})`).join(', '),
              });
            } else {
              message.error(`PID ${dead.join(',')} 已退出，且宿主上暂无活跃 CUDA 进程`);
              return;
            }
          }
        }
      } catch (_) {
        // 查询失败时退化为直接使用用户输入的 PID，让后端来兜错误路径。
      }
    }
    const body = {
      instance: values.instance,
      timeout: values.analysisTimeout,
      iteration_mod: values.iterationModule ?? '',
      iteration_func: values.iterationFunc ?? '',
      iteration_range: values.iterationRange,
      analysis_params: values.analysis_params,
      pids: cleanPids,
      comms: values.comms,
      channel,
      mode,
    };
    const resp: any = await StartAIAnalysis(body);
    if (resp.code === 'Success') {
      const pos = resp.queuePosition ?? resp.data?.queuePosition;
      if (resp.queued ?? resp.data?.queued) {
        message.info(`采集已排队，当前第 ${pos} 位`);
      } else {
        message.success('分析任务已下发');
      }
      refresh();
    } else if (resp.queueFull) {
      message.warning(resp.message ?? '当前采集繁忙，请稍后再试');
    } else {
      message.error(`下发失败：${resp.message ?? 'unknown'}`);
    }
  };

  const formInitial = useMemo(() => ({
    // Prefill instance from VITE_DEFAULT_INSTANCE for fixed demo environments;
    // OSS builds leave it undefined so the user picks from the dropdown.
    instance: import.meta.env.VITE_DEFAULT_INSTANCE || undefined,
    analysis_params: ['adapt'],
    analysisMode: 'duration',
    analysisTimeout: 3000,
    iterationRange: [0, 10],
  }), []);

  return (
    <>
      <Card title="分析参数" style={{ marginBottom: 16 }} bordered={false}>
        <Form
          form={form}
          layout="vertical"
          initialValues={formInitial}
          onFinish={onFinish}
        >
          <Row gutter={16}>
            <Col span={6}>
              <Form.Item name="instance" label="实例 ID"
                rules={[{ required: true, message: '请选择一个已注册的采集端' }]}>
                <Select
                  showSearch
                  placeholder={clients.length ? '选择采集端' : '暂无采集端注册'}
                  options={clients.map(c => {
                    // poll transport 没有常连 WS,wsConnected 恒为 false;此时按 status 判在线
                    const online = c.transport === 'poll' ? c.status !== 'Offline' : c.wsConnected;
                    return {
                      value: c.clientId,
                      label: `${c.clientId}${online ? '' : '（离线）'} — ${c.status}`,
                      disabled: !online,
                    };
                  })}
                  notFoundContent="暂无采集端 — 启动 aiprof-client 后约 5s 出现"
                  onDropdownVisibleChange={(open) => { if (open) refreshClients(); }}
                />
              </Form.Item>
            </Col>
            <Col span={6}>
              <Form.Item
                name="pids"
                label={
                  <span>
                    AI 作业 PID{' '}
                    <Button
                      type="link"
                      size="small"
                      style={{ padding: 0, height: 'auto', fontSize: 12 }}
                      loading={autoLoading}
                      onClick={async () => {
                        const inst = form.getFieldValue('instance');
                        if (!inst) {
                          message.warning('请先选择实例 ID');
                          return;
                        }
                        setAutoLoading(true);
                        try {
                          const resp: any = await ListGpuProcs(inst);
                          if (resp?.code === 'Success' && Array.isArray(resp.data)) {
                            if (!resp.data.length) {
                              message.warning('该实例宿主上暂无活跃 CUDA 进程（nvidia-smi 为空）');
                            } else {
                              const pids = resp.data.map((p: any) => `${p.pid} (${p.name || 'unknown'})`).join(', ');
                              form.setFieldsValue({ pids });
                              const desc = resp.data.map((p: any) => `PID ${p.pid} (${p.name || 'unknown'}, ${p.memMiB || 0} MiB)`).join('; ');
                              message.success(`已自动发现 ${resp.data.length} 个 GPU 进程：${desc}`);
                              message.info('PID 已填入，检查其他参数后点「开始分析」才会真正开始采集', 2.5);
                            }
                          } else {
                            message.error(`自动发现失败：${resp?.message ?? 'unknown'}`);
                          }
                        } finally {
                          setAutoLoading(false);
                        }
                      }}
                    >
                      自动发现
                    </Button>
                  </span>
                }
                tooltip="多个 PID 用逗号分隔；或点「自动发现」拉宿主上活跃的 CUDA 进程；也可留空只按进程名匹配。"
              >
                <Input placeholder="例如 12345 或留空" />
              </Form.Item>
            </Col>
            <Col span={6}>
              <Form.Item name="comms" label="进程名">
                <Input placeholder="例如 python3" />
              </Form.Item>
            </Col>
            <Col span={6}>
              <Form.Item name="analysis_params" label="数据丰富度"
                tooltip="如需 NVTX / NCCL / RDMA / DCGM / ROCm 等更丰富指标，请到阿里云操作系统控制台使用"
                rules={[{ required: true, type: 'array', message: '至少选择一项' }]}>
                <Select
                  mode="multiple"
                  placeholder="选择采集指标"
                  options={[
                    { value: 'adapt', label: '默认采集指标' },
                    { value: 'kernel', label: 'GPU Kernel' },
                    { value: 'pytorch', label: 'PyTorch' },
                    { value: 'python', label: 'Python 调用栈' },
                    { value: 'snapshot', label: 'GPU 显存快照' },
                  ]}
                />
              </Form.Item>
            </Col>
          </Row>
          <Row gutter={16}>
            <Col span={6}>
              <Form.Item name="analysisMode" label="分析模式">
                <Select
                  value={mode}
                  onChange={(v) => setMode(v as any)}
                  options={[
                    { value: 'duration', label: '时长模式' },
                    { value: 'iteration', label: '迭代模式' },
                  ]}
                />
              </Form.Item>
            </Col>
            {mode === 'duration' ? (
              <Col span={6}>
                <Form.Item name="analysisTimeout" label="采集时长（ms）">
                  <InputNumber
                    style={{ width: '100%' }}
                    min={MIN_ANALYSIS_TIME_MS}
                    max={MAX_ANALYSIS_TIME_MS}
                    step={500}
                  />
                </Form.Item>
              </Col>
            ) : (
              <>
                <Col span={6}>
                  <Form.Item name="iterationModule" label="迭代模块"
                    tooltip="迭代入口模块（例如 a.b.module）">
                    <Input placeholder="a.b.module" />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item name="iterationFunc" label="迭代函数"
                    tooltip="迭代入口函数（Class.function）">
                    <Input placeholder="Class.function" />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item label="迭代区间">
                    <Space.Compact>
                      <Form.Item name={['iterationRange', 0]} noStyle>
                        <InputNumber placeholder="起" min={0} />
                      </Form.Item>
                      <Form.Item name={['iterationRange', 1]} noStyle>
                        <InputNumber placeholder="止" min={1} />
                      </Form.Item>
                    </Space.Compact>
                  </Form.Item>
                </Col>
              </>
            )}
          </Row>
          <Row>
            <Col span={24}>
              <Space>
                <Form.Item shouldUpdate noStyle>
                  {() => {
                    const inst = form.getFieldValue('instance');
                    const pids = (form.getFieldValue('pids') || '').toString().trim();
                    const comms = (form.getFieldValue('comms') || '').toString().trim();
                    const disabled = !inst || (!pids && !comms);
                    return (
                      <Button
                        type="primary"
                        htmlType="submit"
                        disabled={disabled}
                        title={disabled ? '需要选择实例且填入 PID 或进程名（避免误采）' : ''}
                      >
                        开始分析
                      </Button>
                    );
                  }}
                </Form.Item>
                <Button onClick={() => form.resetFields()}>重置</Button>
              </Space>
            </Col>
          </Row>
        </Form>
      </Card>

      <Card bordered={false} styles={{ body: { padding: 16 } }}>
        <Tabs
          activeKey={activeTab}
          onChange={(key) => {
            // 外链 tab 从不成为 activeKey，否则第二次点击时 AntD 认为
            // key 没变、不触发 onChange，跳转就失效了。
            if (key === 'aliyun-console') {
              message.info('正在打开阿里云操作系统控制台，首次访问需要登录阿里云账号');
              window.open(ALIYUN_CONSOLE_URL, '_blank', 'noopener');
              return;
            }
            setActiveTab(key);
          }}
          items={[
            {
              key: 'records',
              label: '分析记录',
              children: (
                <AiRecordTable
                  dataSource={records}
                  total={total}
                  loading={loading}
                  onRefresh={refresh}
                  onPageChange={(current, pageSize) => setPagination({ current, pageSize })}
                  onReuseParams={(row) => {
                    const kv: Record<string, any> = {};
                    for (const p of row.parms || []) {
                      const v = typeof p.value === 'object' ? JSON.stringify(p.value) : String(p.value ?? '');
                      if (p.key === 'Params' && v) kv.analysis_params = v.split(/\s+/).filter(Boolean);
                      else if (p.key === 'pids') kv.pids = v;
                      else if (p.key === 'comms') kv.comms = v;
                      else if (p.key === 'timeout') {
                        const n = Number(v);
                        if (!Number.isNaN(n)) kv.analysisTimeout = n;
                      }
                    }
                    kv.instance = row.instance;
                    form.setFieldsValue(kv);
                    message.info('已把该记录的参数填回表单');
                  }}
                />
              ),
            },
            {
              key: 'aliyun-console',
              label: (
                <Tooltip title="在新窗口打开阿里云操作系统控制台的 AI Infra 观测入口，首次访问需要登录阿里云账号">
                  <span>阿里云 GPU 性能与诊断 ↗</span>
                </Tooltip>
              ),
            },
          ]}
        />
      </Card>
    </>
  );
};

export default CapturePage;
