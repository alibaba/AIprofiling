// SPDX-License-Identifier: Apache-2.0
// 类型定义 —— AI observable 页面共享的数据结构。

interface KernelDetails {
  kernel_name: string;
  type_of_operation: string;
  run_times: number;
  use_tensorCore: string;
  total_delay_us: number;
  max_delay_us: number;
  avg_delay_us: number;
  min_delay_us: number;
  block: string;
  grid: string;
  shared_memory_size: number;
}

interface TensorCoresUsage {
  service_time: number;
  unused_time: number;
}

interface MemStatistic {
  Allocated: any[];
  Reserved: any[];
  Time: any[];
}

interface Detail {
  kernel_details: { [key: string]: KernelDetails };
  tensorCores_usage: TensorCoresUsage;
  memory_stistics: MemStatistic;
}

interface DeviceInfo {
  device_name: string;
  memory_size: string;
}

interface GPUUtilization {
  GPU_utilization: number | string;
  SM_utilization: number | string;
  active_blocks_per_SM: number | string;
  active_warps_per_SM: number | string;
}

interface ExecutionDelay {
  cuda_runtime: number;
  kernel: number;
  gpu_memcpy: number;
  total: number;
}

interface Summary {
  conclusion: string;
  device_info: DeviceInfo[];
  GPU_utilization: { [key: string]: GPUUtilization };
  execution_delay: ExecutionDelay;
}

interface Overview {
  detail: Detail;
  summary: Summary;
}

interface AiResult {
  traceUrl: string;
  overview: Overview;
  stackInfo: string;
  mmSnapUrl: string;
  memspikes: GpuMemAllocSpikes;
  memOom: OomOverView;
  memSummary: string;
}

export type OomItem = {
  start_time: number;
  end_time: number;
  size: number;
  frames: string;
};

export type OomOverView = {
  oomItem: OomItem[];
  summary: string;
};

export type GpuMemAllocSpikes = {
  addr: string;
  time: string;
  size: number;
  stack: string;
};

export interface AiDataType {
  [key: string]: AiResult;
}

export interface DiffTableRow {
  name: string;
  before_time: number;
  after_time: number;
  time_diff: number;
  before_time_perc: string;
  after_time_perc: string;
  time_perc_diff: string;
  before_count: number;
  after_count: number;
  count_diff: number;
  before_count_perc: string;
  after_count_perc: string;
  count_perc_diff: string;
}
