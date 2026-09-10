// SPDX-License-Identifier: Apache-2.0
export const analysisParamsLabel: Record<string, string> = {
  adapt: 'Adaptive metrics',
  kernel: 'GPU Kernel',
  pytorch: 'PyTorch',
  python: 'Python Stack',
  snapshot: 'GPU Memory Snapshot',
  cpu: 'CPU Info',
  stack: 'Python Call Stack',
  memory: 'Torch Memory',
};

export const statusColor: Record<string, string> = {
  Success: 'green',
  Done: 'green',
  Failed: 'red',
  Running: 'blue',
  Analyzing: 'blue',
  Pending: 'default',
  Timeout: 'red',
};

// 状态到中文文案 + 图标类型
export const statusMeta: Record<string, { text: string; kind: 'success' | 'error' | 'running' | 'default' }> = {
  // server 已经返回的中文状态（老 list_record 接口）
  '分析成功':  { text: '分析完成', kind: 'success' },
  '分析完成':  { text: '分析完成', kind: 'success' },
  '采集失败':  { text: '采集失败', kind: 'error' },
  '采集超时':  { text: '采集超时', kind: 'error' },
  '分析失败':  { text: '分析失败', kind: 'error' },
  '采集中':    { text: '采集分析中',   kind: 'running' },
  '分析中':    { text: '分析中',   kind: 'running' },
  '排队中':    { text: '排队中',   kind: 'default' },
  // 兜底：英文（走 WS TASK_RESULT 直接落表时可能是英文）
  Success:   { text: '分析完成', kind: 'success' },
  Succeeded: { text: '分析完成', kind: 'success' },
  Done:      { text: '分析完成', kind: 'success' },
  Failed:    { text: '采集失败', kind: 'error' },
  Timeout:   { text: '采集超时', kind: 'error' },
  Running:   { text: '采集中',   kind: 'running' },
  Analyzing: { text: '分析中',   kind: 'running' },
  Pending:   { text: '排队中',   kind: 'default' },
};
