export const NO_AGGREGATION_TRACE_MESSAGE =
    '暂无多进程 GPU Kernel 聚合 TimeLine 数据，请确认任务是否开启 GPU Kernel 采集或稍后重试。';

export function getAggregationTraceUrl(raw: unknown): string {
    if (!raw || typeof raw !== 'object') {
        return '';
    }

    const value = (raw as { aggregationUrl?: unknown }).aggregationUrl;
    if (typeof value !== 'string') {
        return '';
    }

    return value.trim();
}
