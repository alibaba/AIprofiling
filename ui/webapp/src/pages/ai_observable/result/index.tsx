// SPDX-License-Identifier: Apache-2.0
import ProCard from '@ant-design/pro-card';
import React, { useRef, useState, useEffect } from 'react';
import { useLocation } from 'react-router-dom';
import './index.less';
import { AiDataType } from '../component/metaData';
import { GPUKernelBoard } from '../component/gpuKernelBoard';
import { GPUSummaryBoard } from '../component/gpuSummaryBoard';

import GPUMemBoard from '../component/gpuMemBoard';
import type { GpuMemAllocSpikes } from '../component/metaData';
import type { OomOverView } from '../component/metaData';
import ColorBlockTimeline from '../component/gpuOomBoard';
import MemoryProfileChart from '../component/memTimlineChart';

import { GPUTracing } from '../component/perfettoTracing';
import { InitPerfetto, OpenTraceFromUrl } from 'aiprof-perfetto';
import { GetAIQueryResult } from '../api';
import { message, Dropdown, Button, Tabs } from 'antd';
import ComparativeAnalysis from '../component/comparativeAnalysis';
import { useNavigate } from 'react-router-dom';
import ReportLineChart from './ReportLineChart'
import { useUpdateEffect } from 'ahooks';
import pako from 'pako';
import MemoryViz from '../component/MemoryViz';
import { withBase } from '../../../utils/basePath';

interface DiagnoseResultCardProp {
    key: string;
    cardTitle: string;
    cardContent: React.ReactNode;
    isExtra: boolean;
}
const TracingStyle: React.CSSProperties = {
    backgroundColor: 'white',
    color: 'black',
}
const AiAnalysisResult: React.FC = () => {
    const location = useLocation();
    const queryParams = new URLSearchParams(location.search);
    const pid = queryParams.get('pid') || '';
    const analysisIds = queryParams.get('analysisId');
    const embedMode = queryParams.get('embed') || '';
    const [analysisId, setAnalysisId] = useState<string | null>(null);
    const [diagnoseResultCardList, setDiagnoseResultCardList] = useState<DiagnoseResultCardProp[]>([],);
    const [perfettoInit, setPerfettoInit] = useState(0);
    const [tracingFlag, setTracingFlag] = useState(false);
    const perfettoInitRef = useRef(false); // 防止重复初始化
    const [showPid, setShowPid] = useState<string>('');
    const [pidList, setPidList] = useState<string[]>([]);
    const showPidRef = useRef(showPid);
    const [AIAnalysisResultData, setAIAnalysisResultData] = useState<AiDataType>();
    const AIAnalysisResultDataRef = useRef(AIAnalysisResultData);

    const [memResultCardList, setmemResultCardList] = useState<DiagnoseResultCardProp[]>([],);
    const [memSnapUrl, setMemSnapUrl] = useState<string>('');
    const memSnapRef = useRef(memSnapUrl)
    const [memSummary, setMemSummary] = useState<string>('');
    const [memTmlineUrl, setMemTmlineUrl] = useState<string>('');
    const [tabListData, setTabListData] = useState<any[]>([]);


    const [reportMemUsageData, setReportMemUsageData] = useState<any>({
        data: [],
    });

    const navigate = useNavigate();

    const contentMap = {
        menu2:
            <div className="container">
                <ProCard>
                    <h1> 当前进程:  {showPidRef.current}</h1>
                </ProCard>
                {memResultCardList.map((item) => {
                    return (
                        <ProCard
                            key={item.key}
                            title={
                                <span className="title">{item.cardTitle}</span>
                            }
                        >
                            {item.cardContent}
                        </ProCard>
                    );
                })}
                <ProCard
                    title={
                        <span className="title">{"GPU Memory SnapShot分析"}</span>
                    }
                >
                    <div
                        style={{
                            border: '1px solid #ccc',
                            padding: '10px',
                            borderRadius: '8px',
                            backgroundColor: '#f9f9f9',
                            height: '700px', // 设置一个高度，iframe 才能显示
                        }}
                    >
                      {memSnapUrl && memSnapUrl != '' && <MemoryViz snapshotUrl={memSnapUrl} />}
                    </div>
                </ProCard>
            </div>,
        menu1: <div className="container">
            <ProCard>
                <h1>当前进程: {showPidRef.current}</h1>
            </ProCard>
            {
                diagnoseResultCardList.map((item) => {
                    return (
                        <ProCard
                            key={item.key}
                            title={
                                <span className="title">{item.cardTitle}</span>
                            }
                        >
                            {item.cardContent}
                        </ProCard>
                    );
                })}
            <ProCard
                key={"tracing"}
                title={
                    <span className="title">{"CPU/GPU Tracing分析"}</span>
                }
                loading={!tracingFlag}
                bodyStyle={TracingStyle}
            >
                <GPUTracing />
            </ProCard>
        </div>
    }
    const tabList = [
        {
            label: 'AI Profiling',
            key: 'Profiling',
            children: <>{contentMap.menu1}</>
        },
        {
            label: 'GPU 显存快照',
            key: 'SnapShot',
            children: <>{contentMap.menu2}</>
        },
    ]
    //处理显示tab
    useUpdateEffect(() => {
        const pidData: any = findByKeyIncludes(AIAnalysisResultData, pid)
        if (pidData && pidData?.traceUrl && !pidData?.mmSnapUrl) {
            setTabListData([{
                label: 'AI Profiling',
                key: 'Profiling',
                children: <>{contentMap.menu1}</>
            },])
        } else if (pidData && pidData?.mmSnapUrl && !pidData?.traceUrl) {
            setTabListData([{
                label: 'GPU 显存快照',
                key: 'SnapShot',
                children: <>{contentMap.menu2}</>
            },])
        } else if (pidData && pidData?.mmSnapUrl && pidData?.traceUrl) {
            setTabListData(tabList)
        }
    }, [memResultCardList, showPid, diagnoseResultCardList, TracingStyle])
    // 获取pid对应的数据
    const findByKeyIncludes = (objData: any, substr: string) => {
        if (!objData) return null;
        
        const keys = Object.keys(objData).filter(key => key !== 'aggregationUrl');
        
        if (!substr || substr.trim() === '') {
            return keys.length > 0 ? objData[keys[0]] : null;
        }
        
        for (const key of keys) {
            if (key.includes(substr)) {
                return objData[key];
            }
        }
        return null;
    }

    useEffect(() => {
        setAnalysisId(analysisIds)
        fetchAIQueryResult(analysisIds);
    }, [analysisIds]);
    // 根据该analysisId获取数据
    const fetchAIQueryResult = async (analysisId: any) => {
        try {
            //TODO: FIXME:
            const response: any = await GetAIQueryResult(analysisId);
            if (response.code == "Success") {
                const data: AiDataType = JSON.parse(response.data);
                AIAnalysisResultDataRef.current = data;
                setAIAnalysisResultData(data);
            } else {
                const msg = "获取AI分析结果失败: " + response.message;
                message.error(msg);
            }
        } catch (error) {
            console.log('error', error);
        }
    };
    useEffect(() => {
        handleData(AIAnalysisResultData)
    }, [AIAnalysisResultDataRef, AIAnalysisResultData])

    const handleData = async (analysisResultData: any) => {
        if (!analysisResultData) return;

        const pids = Object.keys(analysisResultData).filter(key => key !== 'aggregationUrl');

        let current;
        if (pid && pid.trim() !== '') {
            current = pids.find(item => item.includes(pid));
        } else {
            current = pids[0];
        }
        
        if (pids.length > 0 && current) {
            if (current) {
                showPidRef.current = current;
                setShowPid(current)
            }
            setPidList(pids);
            setMemSnapUrl(analysisResultData[showPidRef.current].mmSnapUrl)
            setMemSummary(analysisResultData[showPidRef.current].memSummary)
            setMemTmlineUrl(analysisResultData[showPidRef.current].memTmlineUrl)
            let memUseData = []
            let legend = []
            const memAllocatedData = analysisResultData[showPidRef.current].overview.detail.memory_stistics.Allocated
            const memReservedData = analysisResultData[showPidRef.current].overview.detail.memory_stistics.Reserved
            const memXdata = analysisResultData[showPidRef.current].overview.detail.memory_stistics.Time
            let memUseData0: { name: string; type: string; stack: string; data: (string | number)[][] } = {
                name: "Reserved",
                type: 'line',
                stack: 'Total',
                data: memReservedData
            }
            let memUseData1: { name: string; type: string; stack: string; data: (string | number)[][] } = {
                name: "Allocated",
                type: 'line',
                stack: 'Total',
                data: memAllocatedData
            }
            memUseData.push(memUseData0)
            memUseData.push(memUseData1)
            legend.push("Reserved")
            legend.push("Allocated")

            setReportMemUsageData(
                {
                    xdata: memXdata,
                    data: memUseData,
                    legend: legend,
                }
            );
        } else {
            setShowPid("暂无数据");
        }
        
        // 检查 traceUrl 是否存在
        const traceUrl = analysisResultData[showPidRef.current]?.traceUrl;
        
        if (traceUrl && !perfettoInitRef.current) {
            perfettoInitRef.current = true; // 标记已初始化
            try {
                await InitPerfetto({
                    callback: () => {
                        setTracingFlag(true);
                    }
                }, "");
                let fullUrl = traceUrl;
                if (traceUrl.startsWith('/')) {
                    fullUrl = window.location.origin + traceUrl;
                }
                OpenTraceFromUrl(fullUrl);
                // 延迟设置状态，确保 Perfetto 完全初始化
                setTimeout(() => {
                    setPerfettoInit(1);
                }, 500);
            } catch (error: any) {
                console.warn('Index页面 - InitPerfetto 失败（可能已被初始化）:', error.message);
                setTimeout(() => {
                    setPerfettoInit(1);
                    setTracingFlag(true);
                }, 500);
            }
        } else if (traceUrl && perfettoInitRef.current) {
            console.log('Index页面 - Perfetto 已经初始化，跳过');
            // 如果已经初始化，直接触发加载
            if (perfettoInit === 0) {
                setTimeout(() => {
                    setPerfettoInit(1);
                }, 500);
            }
        } else {
            console.warn('Index页面 - traceUrl 为空，跳过 Perfetto 初始化');
        }
    }


    const [memProfData, setData] = useState<{
        timestamps: number[];
        values: number[][];
    } | null>(null);
    const [isLoading, setIsLoading] = useState<boolean>(true);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        const fetchData = async () => {
        if (!memTmlineUrl) {
            setIsLoading(false);
            setError(null);
            setData(null);
            return;
        }
        try {
            setIsLoading(true);
            const response = await fetch(memTmlineUrl);
            if (!response.ok) {
            throw new Error(`HTTP Error: ${response.status} ${response.statusText}`);
            }
            const arrayBuffer = await response.arrayBuffer();
            const uint8Arr = new Uint8Array(arrayBuffer);
            const decompressed = pako.ungzip(uint8Arr, { to: 'string' });
            const json = JSON.parse(decompressed);
            // 校验数据结构 [ [ts], [[v],...] ]
            if (Array.isArray(json) && json.length >= 2) {
            setData({
                timestamps: json[0],
                values: json[1]
            });
            } else {
            throw new Error("Invalid JSON structure received from URL");
            }
        } catch (err: any) {
            console.error("Failed to fetch memory data:", err);
            setError(err.message || "Unknown error occurred");
        } finally {
            setIsLoading(false);
        }
        };
        fetchData();
    }, [memTmlineUrl]);

    useEffect(() => {
        // 移除了 setPerfettoInit(perfettoInit + 1); 以防止重复加载
        try {
            if (AIAnalysisResultDataRef.current && showPidRef.current) {
                const memSumData = AIAnalysisResultDataRef.current[showPidRef.current].memSummary ?
                    AIAnalysisResultDataRef.current[showPidRef.current].memSummary : "暂无建议";

                setmemResultCardList([
                    {
                        key: 'gmem_cause',
                        cardTitle: '分析建议',
                        cardContent: (
                            <div style={{ fontSize: '12px', lineHeight: '20px' }}>
                                {memSumData}
                            </div>
                        ),
                        isExtra: false,
                    },
                    {
                        key: 'gpu_mem_usage',
                        cardTitle: 'GPU显存碎片情况',
                        cardContent: (
                            <ReportLineChart title={'显存使用情况'} id={'TEST'} chartData={reportMemUsageData} />
                        ),
                        isExtra: true,
                    },
                    // {
                    //     key: 'gmem_profil_stage',
                    //     cardTitle: '显存各阶段使用分析',
                    //     cardContent: (
                    //         //f9fafb
                    //         <div style={{ padding: '30px', backgroundColor: '#11111a', minHeight: '100vh' }}>
                    //             <div style={{ maxWidth: '1200px', margin: '0 auto' }}>
                    //                 {isLoading && (
                    //                 <div style={{ padding: '100px', textAlign: 'center', backgroundColor: '#222' }}>                                  
                    //                     <p>ðLoading data from remote server...</p>
                    //                 </div>
                    //                 )}
                    //                 {error && (
                    //                 <div style={{ padding: '20px', backgroundColor: '#fee2e2', color: '#b91c1c', borderRadius: '8px' }}>
                    //                     <strong>Error:</strong> {error}
                    //                     <button
                    //                     onClick={() => window.location.reload()}
                    //                     style={{ marginLeft: '15px', cursor: 'pointer', border: 'none', background: 'none', textDecoration: 'underline' }}
                    //                     >
                    //                     Retry
                    //                     </button>
                    //                 </div>
                    //                 )
                    //                 }
                    //                 {memProfData && !isLoading && (
                    //                 <div style={{
                    //                     backgroundColor: '#111',
                    //                     borderRadius: '12px',
                    //                     boxShadow: '0 4px 6px -1px rgba(0, 0, 0, 0.1)',
                    //                     padding: '15px'
                    //                 }}>
                    //                     <MemoryProfileChart
                    //                     rawTimestamps={memProfData.timestamps}
                    //                     rawValues={memProfData.values}
                    //                     />
                    //                     <div style={{ padding: '10px', fontSize: '12px', color: '#9ca3af' }}>
                    //                     Data source: {memoryRawData}
                    //                     </div>
                    //                 </div>
                    //                 )}
                    //             </div>
                    //         </div>),
                    //     isExtra: false,
                    // },
                ]);
            }

            if (AIAnalysisResultDataRef.current && showPidRef.current) {
                const conclusion = AIAnalysisResultDataRef.current[showPidRef.current].overview?.summary?.conclusion ?
                    AIAnalysisResultDataRef.current[showPidRef.current].overview.summary.conclusion :
                    '暂无内容';
                const summary = AIAnalysisResultDataRef.current[showPidRef.current].overview?.summary ?
                    AIAnalysisResultDataRef.current[showPidRef.current].overview.summary :
                    {
                        conclusion: "",
                        device_info: [],
                        GPU_utilization: {
                            GPU_total: {
                                GPU_utilization: 0,
                                SM_utilization: 0,
                                active_blocks_per_SM: 0,
                                active_warps_per_SM: 0
                            }
                        },
                        execution_delay: {
                            cuda_runtime: 0,
                            kernel: 0,
                            Trace: 0,
                            total: 0,
                        }

                    };
                const detail = AIAnalysisResultDataRef.current[showPidRef.current].overview?.detail ?
                    AIAnalysisResultDataRef.current[showPidRef.current].overview.detail :
                    {
                        kernel_details: {},
                        tensorCores_usage: {
                            service_time: 0,
                            unused_time: 0
                        }
                    };
                const stackInfo = AIAnalysisResultDataRef.current[showPidRef.current].stackInfo ?
                    AIAnalysisResultDataRef.current[showPidRef.current].stackInfo :
                    {
                        stackInfo: ""
                    };

                setDiagnoseResultCardList([
                    {
                        key: 'cause',
                        cardTitle: '分析建议',
                        cardContent: (
                            <div style={{ fontSize: '12px', lineHeight: '20px' }}>
                                {conclusion}
                            </div>
                        ),
                        isExtra: false,
                    },
                    {
                        key: 'gpu_summary',
                        cardTitle: 'CPU/GPU Summary',
                        cardContent: (
                            <GPUSummaryBoard
                                summary={summary}
                                showpid={showPid}
                            />
                        ),
                        isExtra: true,
                    },
                    {
                        key: 'gpu_kernel',
                        cardTitle: 'GPU Kernel分析',
                        cardContent: (
                            <GPUKernelBoard
                                detail={detail}
                                stackInfo={stackInfo}
                                showpid={showPid}
                                analysisId={analysisId}
                                totalTime={summary.execution_delay.total}
                            />
                        ),
                        isExtra: true,
                    },
                    {
                        key: 'CPU/GPU_Tracing',
                        cardTitle: '迭代统计与差分分析',
                        cardContent: (
                            <ComparativeAnalysis resultData={AIAnalysisResultDataRef?.current || null} pid={pid} analysisId={analysisId} />
                        ),
                        isExtra: true,
                    },
                ]);
            }
        } catch (error) {
            console.log('error', error);
        }
    }, [tracingFlag, showPid]);


    return (
        <>
            {embedMode === 'memviz' ? (
                <div style={{ padding: 12, background: '#f9f9f9', minHeight: '100vh' }}>
                    {memSnapUrl && memSnapUrl !== '' ? (
                        <div style={{ border: '1px solid #ccc', padding: 10, borderRadius: 8, backgroundColor: '#f9f9f9', height: 'calc(100vh - 40px)' }}>
                            <MemoryViz snapshotUrl={memSnapUrl} />
                        </div>
                    ) : (
                        <div style={{ padding: 20, color: '#5f6368' }}>
                            {analysisId ? '正在加载 CUDA 显存快照…' : '缺少 analysisId'}
                        </div>
                    )}
                </div>
            ) : (
                <>
                    <div>
                        <Button onClick={() => {
                            window.location.href = withBase('/ai_observable/result/Report?analysisId=' + analysisId)
                        }}> 返回首页 </Button>
                    </div>
                    <Tabs
                        type="line"
                        items={tabListData}
                        destroyOnHidden={true}
                    />
                </>
            )}
        </>
    );
};
export default AiAnalysisResult;
