import React, { useEffect, useState } from 'react';
import ReportTable from './ReportTable';
import ProCard from '@ant-design/pro-card';
import { Alert, Col, Row } from 'antd';
import './index.less';
import ReportLineChart from './ReportLineChart';
import { InitPerfetto, OpenTraceFromUrl } from 'aiprof-perfetto';
import { GPUTracing } from '../component/perfettoTracing';
import { GetAIQueryResult } from '../api';
import { getAggregationTraceUrl, NO_AGGREGATION_TRACE_MESSAGE } from './reportData';

const TracingStyle: React.CSSProperties = {
    backgroundColor: 'white',
    color: 'black',
};

type TraceStatus = 'loading' | 'ready' | 'empty' | 'error';

const Report: React.FC = () => {
    const queryParams = new URLSearchParams(location.search);
    const analysisId = queryParams.get('analysisId');
    const [traceStatus, setTraceStatus] = useState<TraceStatus>('loading');
    const [traceMessage, setTraceMessage] = useState<string>('');
    const [reportTableData, setReportTableData] = useState<any[]>([]);
    const [reportAllocatedData, setReportAllocatedData] = useState<any>({
        data: [],
    });
    const [reportReservedData, setReportReservedData] = useState<any>({
        data: [],
    });

    useEffect(() => {
        initData();
    }, [analysisId]);

    const initData = async () => {
        setTraceStatus('loading');
        setTraceMessage('');

        if (!analysisId) {
            setTraceStatus('error');
            setTraceMessage('缺少 analysisId，无法获取多进程 GPU Kernel 聚合 TimeLine 数据。');
            return;
        }

        try {
            const res: any = await GetAIQueryResult(analysisId);
            const { code, data, message } = res;
            if (code !== 'Success') {
                setTraceStatus('error');
                setTraceMessage(message || '获取多进程 GPU Kernel 聚合 TimeLine 数据失败。');
                return;
            }

            const raw = JSON.parse(data);
            if (!raw || typeof raw !== 'object') {
                setTraceStatus('error');
                setTraceMessage('AI 分析结果格式异常，无法读取 TimeLine 数据。');
                return;
            }

            const urlData = getAggregationTraceUrl(raw);

            //处理表格数据
            const handleData = [];
            const allocate_data = [];
            const reserve_data = [];
            const legend = [];
            let xdata = [];

            for (const key in raw) {
                if (key === 'aggregationUrl') {
                    continue;
                }

                let pid = '';
                if (Object.prototype.hasOwnProperty.call(raw, key)) {
                    const result: any = parseData(key);
                    pid = result.pid;
                    const mem_total = raw[key]?.overview?.summary?.device_info?.[0]?.memory_size || 0;
                    const mem_used = raw[key]?.overview?.summary?.device_info?.[0]?.memory_used || 0;
                    if (mem_total == 0) {
                        result.memory_rate = '';
                        result.total_memory = '';
                    } else {
                        result.memory_rate = (parseFloat((mem_used / mem_total).toFixed(2)) * 100).toString() + '%';
                        result.total_memory = mem_total;
                    }
                    result.device = raw[key]?.overview?.summary?.device_info?.[0]?.device_name || '';
                    result.GPUpid = result.gpu;
                    handleData.push({
                        ...result,
                        analysisId: analysisId,
                    });
                }

                const allocate_series: { name: string; type: string; stack: string; data: (string | number)[][] } = {
                    name: 'pid: ' + pid,
                    type: 'line',
                    stack: 'Total',
                    data: raw[key]?.overview?.detail?.memory_stistics?.Allocated || [],
                };

                const reserve_series: { name: string; type: string; stack: string; data: (string | number)[][] } = {
                    name: 'pid: ' + pid,
                    type: 'line',
                    stack: 'Total',
                    data: raw[key]?.overview?.detail?.memory_stistics?.Reserved || [],
                };

                legend.push('pid: ' + pid);
                allocate_data.push(allocate_series);
                reserve_data.push(reserve_series);
                xdata = raw[key]?.overview?.detail?.memory_stistics?.Time || [];
            }
            setReportTableData(handleData);

            // GPU Memory Allocated Line
            setReportAllocatedData({
                xdata: xdata,
                data: allocate_data,
                legend: legend,
            });

            // GPU Memory Reserved Line
            setReportReservedData({
                xdata: xdata,
                data: reserve_data,
                legend: legend,
            });

            if (!urlData) {
                setTraceStatus('empty');
                setTraceMessage(NO_AGGREGATION_TRACE_MESSAGE);
                return;
            }

            let hasHandledTrace = false;
            const openTrace = () => {
                if (hasHandledTrace) {
                    return;
                }
                hasHandledTrace = true;

                try {
                    OpenTraceFromUrl(urlData);
                    setTraceStatus('ready');
                } catch (error: any) {
                    setTraceStatus('error');
                    setTraceMessage(error?.message || '加载多进程 GPU Kernel 聚合 TimeLine 失败。');
                }
            };

            let timeoutId: number | undefined;
            const initTimeout = new Promise((_, reject) => {
                timeoutId = window.setTimeout(() => reject(new Error('Perfetto 初始化超时')), 30000);
            });

            try {
                await Promise.race([
                    InitPerfetto({
                        callback: openTrace,
                    }, ''),
                    initTimeout,
                ]);
            } finally {
                if (timeoutId) {
                    window.clearTimeout(timeoutId);
                }
            }

            openTrace();
        } catch (error: any) {
            setTraceStatus('error');
            setTraceMessage(error?.message || '加载多进程 GPU Kernel 聚合 TimeLine 失败。');
        }
    };

    //解析数据
    const parseData = (str: string) => {
        // 清理字符串前后空格
        const cleaned = str.trim();
        // 正则匹配：数字(冒号)[可选GPU标识]剩余内容
        const regex = /^(\d+):\s*(?:\[([^\]]+)\]\s*)?(.*)$/;
        const match = cleaned.match(regex);
        if (!match) return { pid: '', gpu: '', comm: '' }; // 无效格式返回空数组
        return {
            pid: match[1],
            gpu: match[2] || '', // 如果没有GPU标识则为空字符串
            comm: match[3].trim(), // 清理comm部分的前后空格
        };
    };

    const renderTraceContent = () => {
        if (traceStatus === 'ready') {
            return <GPUTracing />;
        }

        if (traceStatus === 'empty') {
            return <Alert type="info" showIcon message={traceMessage || NO_AGGREGATION_TRACE_MESSAGE} />;
        }

        if (traceStatus === 'error') {
            return <Alert type="warning" showIcon message="TimeLine 加载失败" description={traceMessage} />;
        }

        return null;
    };

    return (
        <div className="reportPage">
            <ProCard
                key="reportTable"
                title="进程基础信息概览"
            >
                <ReportTable dataSource={reportTableData} />
            </ProCard>

            <ProCard
                key="reportLineChart"
                title="多卡显存概览"
            >
                {reportAllocatedData?.data[0]?.data?.length > 0 || reportReservedData?.data[0]?.data?.length > 0 ? <Row gutter={[16, 16]} >
                    {reportAllocatedData?.data[0]?.data && reportAllocatedData?.data[0]?.data?.length > 0 && <Col xxl={reportReservedData?.data[0]?.data?.length > 0 ? 12 : 24} xl={24} lg={24} sm={24} xs={24} >
                        <ReportLineChart title="多卡 Memory Allocated 统计" id="Allocated" chartData={reportAllocatedData} />
                    </Col >}
                    {reportReservedData?.data[0]?.data && reportReservedData?.data[0]?.data?.length > 0 && <Col xxl={reportAllocatedData?.data[0]?.data?.length > 0 ? 12 : 24} xl={24} lg={24} sm={24} xs={24} >
                        <ReportLineChart title="多卡 Memory Reserved 统计" id="Reserved" chartData={reportReservedData} />
                    </Col >}
                </Row > :
                    <div>暂无数据</div>}
            </ProCard>
            <ProCard
                key="reportTracing"
                title="多进程 GPU Kernel 聚合分析 TimeLine"
                loading={traceStatus === 'loading'}
                bodyStyle={TracingStyle}
            >
                {renderTraceContent()}
            </ProCard>
        </div>
    );
};

export default Report;
