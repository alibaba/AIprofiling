import { message, Table, TableProps, Modal, Form, Row, Col, Spin } from 'antd';
import { useIntl } from 'react-intl';
import { GetListRecord, GetAIQueryResult, GetAIDiffAnalysisResult } from './api'
import ProCard from '@ant-design/pro-card';
import './result/index.less';
import { ProForm, ProFormText, ProFormSelect, ProFormGroup } from '@ant-design/pro-components';
import { GPUTracing } from './component/perfettoTracing';
import React, { useRef, useState, useEffect } from 'react';
import { AiDataType, DiffTableRow } from './component/metaData';
import { InitPerfetto, OpenTraceFromUrl } from 'aiprof-perfetto';
import { AiRecordTable } from './component/aiRecordTable';
import * as echarts from 'echarts';
import { autoTimeConversion } from '@/utils/automaticUnitConversion';

const TracingStyle: React.CSSProperties = {
    backgroundColor: 'white',
    color: 'black',
}
const StepInfoBarChartNew = ({ stepInfo }: any) => {
    let StepInfoBarChartNewContainer = useRef(null);
    var option;
    var rawData = [stepInfo.kernel, stepInfo.cudaRuntime, stepInfo.other];
    console.log("rawData", rawData)
    // const series = [
    //     'kernel',
    //     'cuda_runtime',
    //     'other',
    //   ].map((name, sid) => {
    //     return {
    //       name,
    //       type: 'bar',
    //       stack: 'total',
    //       barWidth: '60%',
    //       label: {
    //         show: true,
    //         formatter: (params) => Math.round(params.value * 1000) / 10 + '%'
    //       },
    //       data: rawData[sid].map((d, did) =>
    //         stepInfo.dur[did] <= 0 ? 0 : d / stepInfo.dur[did]
    //       )
    //     };
    //   });
    useEffect(() => {
        const colors = ['#91CC75', '#fac858', '#EE6666', '#fc8452', '#9a60b4', '#ea7ccc'];

        const chartInstance = echarts.init(StepInfoBarChartNewContainer.current);
        option = {
            color: colors,
            tooltip: {
                trigger: 'axis',
                axisPointer: {
                    type: 'cross'
                }
            },
            legend: {
                textStyle: {
                    color: 'black'
                }
            },
            xAxis: [
                {
                    type: 'category',
                    axisLabel: {
                        textStyle: {
                            color: 'black'
                        }
                    },
                    data: stepInfo.id
                }
            ],
            yAxis: [
                {
                    name: "Step耗时/微秒",
                    nameTextStyle: {
                        color: 'black',
                    },
                    type: 'value',
                    splitLine: {
                        show: false
                    },
                    axisLabel: {
                        textStyle: {
                            color: 'black'
                        }
                    }
                },
            ],
            series: [
                {
                    name: 'kernel',
                    type: 'bar',
                    stack: 'time',
                    data: rawData[0].map((d: number) => {
                        return d.toFixed(2)
                    }),
                    emphasis: {
                        focus: 'series'
                    },
                },
                {
                    name: 'cudaRuntime',
                    type: 'bar',
                    stack: 'time',
                    data: rawData[1].map((d: number) => {
                        return d.toFixed(2)
                    }),
                    emphasis: {
                        focus: 'series'
                    },
                },
                {
                    name: 'other',
                    type: 'bar',
                    stack: 'time',
                    data: rawData[2].map((d: number) => {
                        return d.toFixed(2)
                    }),
                    emphasis: {
                        focus: 'series'
                    },
                },
            ]
        };
        chartInstance.setOption(option);

        return () => {
            if (chartInstance) {
                chartInstance.dispose();
            }
        };
    }, [stepInfo]);

    return <div ref={StepInfoBarChartNewContainer} style={{ width: '100%', height: '400px' }}></div>;
};

const StepLossBarChart = ({ stepInfo }: any) => {
    const chartRef = useRef(null);
    var option;
    var rawData = [stepInfo.loss];
    useEffect(() => {
        const colors = ['#ea7ccc'];

        const chartInstance = echarts.init(chartRef.current);
        option = {
            color: colors,
            tooltip: {
                trigger: 'axis',
                axisPointer: {
                    type: 'cross'
                }
            },
            legend: {
                textStyle: {
                    color: 'black'
                }
            },
            xAxis: [
                {
                    type: 'category',
                    axisLabel: {
                        textStyle: {
                            color: 'black'
                        }
                    },
                    data: stepInfo.id
                }
            ],
            yAxis: [
                {
                    name: "Loss值",
                    nameTextStyle: {
                        color: 'black',
                    },
                    type: 'value',
                    splitLine: {
                        show: false
                    },
                    axisLabel: {
                        textStyle: {
                            color: 'black'
                        }
                    }
                },
            ],
            series: [
                {
                    name: 'loss',
                    type: 'bar',
                    stack: 'time',
                    data: rawData[0].map((d: number) => {
                        return d.toFixed(2)
                    }),
                    emphasis: {
                        focus: 'series'
                    },
                }
            ]
        };
        chartInstance.setOption(option);

        return () => {
            if (chartInstance) {
                chartInstance.dispose();
            }
        };
    }, [stepInfo]);

    return <div ref={chartRef} style={{ width: '100%', height: '400px' }}></div>;

};

interface DiffViewerProps {
    diffTable: DiffTableRow[];
    showDetails?: boolean;
}

const processStepData = (steps: any[]) => {
    const id: string[] = [];
    const dur: number[] = [];
    const loss: number[] = [];
    const start: number[] = [];
    const kernel: number[] = [];
    const cudaRuntime: number[] = [];
    const other: number[] = [];
    // 累加 kernel 和 cuda_runtime 值

    steps.forEach((step, index) => {
        id.push(`step_${index}`); // 生成唯一标识
        let duration = step.dur == 'N/A' ? 0 : step.dur;
        dur.push(step.dur == 'N/A' ? 0 : step.dur); // 提取 dur 值
        loss.push(step.loss == 'N/A' ? 0 : step.loss); // 提取 loss 值
        start.push(step.start == 'N/A' ? 0 : step.start); // 提取 dur 值

        let kernelSum = 0;
        let cudaRuntimeSum = 0;
        for (const key in step) {
            if (key.startsWith('kernel:')) {
                kernelSum += step[key]; // 累加 kernel:XX 的值
            } else if (key.startsWith('cuda_runtime:')) {
                cudaRuntimeSum += step[key]; // 累加 cuda_runtime:XX 的值
            }
        }
        let value = (duration - kernelSum - cudaRuntimeSum) < 0 ?
            0 : (duration - kernelSum - cudaRuntimeSum).toFixed(2);
        other.push(Number(value))
        kernel.push(kernelSum < 0 ? 0 : kernelSum); // 将 kernel 总和存入数组
        cudaRuntime.push(cudaRuntimeSum < 0 ? 0 : cudaRuntimeSum); // 将 cuda_runtime 总和存入数组
    });

    return { id, dur, loss, start, kernel, cudaRuntime, other };
};



const DiffViewer: React.FC<DiffViewerProps> = ({
    diffTable,
    showDetails = false
}) => {
    const columns: TableProps<DiffTableRow>['columns'] = [
        {
            title: () => <span style={{ fontSize: '20px' }}>名称</span>,
            dataIndex: 'name',
            key: 'name',
            render: (_, record) => (
                <div>
                    <div style={{ fontSize: '18px' }}>{record.name}</div>
                </div>
            ),
            width: 500,
        },
        {
            title: () => <span style={{ fontSize: '20px' }}>基线Step</span>,
            dataIndex: 'before_time',
            key: 'before_time',
            render: (_, record: any) => (
                <div>
                    {showDetails && <div style={{ fontSize: '16px' }}>耗时: {autoTimeConversion(record?.before_time)}, 占比: {(record.before_time_perc * 100).toFixed(2)}%</div>}
                    <div style={{ fontSize: '16px' }}>调用: {record.before_count}, 占比: {(record.before_count_perc * 100).toFixed(2)}%</div>
                </div>
            ),
        },

        {
            title: () => <span style={{ fontSize: '20px' }}>对比Step</span>,
            dataIndex: 'after_time',
            key: 'after',
            render: (_, record: any) => (
                <div>
                    {showDetails && <div style={{ fontSize: '16px' }}>耗时: {autoTimeConversion(record?.after_time)} , 占比: {(record.after_time_perc * 100).toFixed(2)}%</div>}
                    <div style={{ fontSize: '16px' }}>调用: {record.after_count}, 占比:{(record.after_count_perc * 100).toFixed(2)}%</div>
                </div>
            ),
        },
        {
            title: () => <span style={{ fontSize: '20px' }}>耗时差异</span>,
            dataIndex: 'time_diff',
            key: 'time_diff',
            sorter: (a, b) => Number(a.time_diff - b.time_diff),

            render: (_, record:any) => (
                <div>
                    {showDetails && <div style={{ fontSize: '16px', color: record.time_diff && record.time_diff > 0 ? 'red' : (record.time_diff < 0 ? 'green' : 'black') }}>数值: {autoTimeConversion(record?.time_diff)}, 占比: {(record.time_perc_diff * 100).toFixed(2)}%</div>}
                </div>
            ),
        },
        {
            title: () => <span style={{ fontSize: '20px' }}>调用差异</span>,
            dataIndex: 'count_diff',
            key: 'count_diff',
            sorter: (a, b) => Number(a.count_diff - b.count_diff),
            render: (_, record:any) => (
                <div>
                    {showDetails && <div style={{ fontSize: '16px', color: record.count_diff && record.count_diff > 0 ? 'red' : (record.count_diff < 0 ? 'green' : 'black') }}>数值: {autoTimeConversion(record?.count_diff)}, 占比: {(record.count_perc_diff * 100).toFixed(2)}%</div>}
                </div>
            ),
        },
    ];

    // 渲染组件
    return (
        <div className="diff-viewer-container">
            <Table
                dataSource={diffTable}
                columns={columns}
                rowKey="name"
                bordered
                pagination={{ pageSize: 10 }}
            />
        </div>
    );
};

export const DiffAnalysisPage = () => {
    const intl = useIntl();
    const [totalRecord, setTotalRecord] = useState<number>(0);
    const [listRecordTableData, setListRecordTableData] = useState([]);
    const [currentPage, setCurrentPage] = useState<number>(1);
    const [pageSize, setPageSize] = useState<number>(10);
    const [AIAnalysisResultData, setAIAnalysisResultData] = useState<AiDataType>();
    const AIAnalysisResultDataRef: any = useRef(AIAnalysisResultData);
    const [showPid, setShowPid] = useState<string>('');
    const showPidRef = useRef(showPid);
    const [perfettoInit, setPerfettoInit] = useState(0);
    const [modalIsOpen, setModalIsOpen] = useState(false);
    const [tracingFlag, setTracingFlag] = useState<boolean>(false);
    const [stepInfo, setStepInfo] = useState<any>([]);
    const [selectedData, setSelectedData] = useState<{
        pid?: string;
        stepBaseline?: string;
        stepDiff?: string;
        analysisId?: string;
    }>({});


    const [diffResultData, setDiffResultData] = useState([]);
    const [showDiffModal, setShowDiffModal] = useState<boolean>(false);
    const [loadingModal, setLoadingModal] = useState<boolean>(false);
    const [showEchart, setShowEchart] = useState<boolean>(false);

    useEffect(() => {
        try {
            if (AIAnalysisResultDataRef.current && showPidRef.current) {
                const stepInfo: any = processStepData(AIAnalysisResultDataRef.current[showPidRef.current].stepInfo);
                const hasData = checkAnyArrayNotEmpty(stepInfo)
                setShowEchart(hasData)
                setStepInfo(stepInfo);
            }
        } catch (error) {
            console.log('error', error);
        }
    }, [perfettoInit]);
    const checkAnyArrayNotEmpty = (data: any) => {
        // 遍历所有键
        for (const key in data) {
            if (
                Array.isArray(data[key]) &&  // 确保是数组
                data[key].length > 0         // 数组长度大于0
            ) {
                return true; // 存在非空数组，直接返回true
            }
        }
        return false; // 所有数组均为空
    }

    useEffect(() => {
        handleListRecord();
        const intervalId = setInterval(handleListRecord, 10000);
        return () => clearInterval(intervalId);
    }, [currentPage, pageSize]);

    const handleSelectChange = (type: 'pid' | 'stepBaseline' | 'stepDiff' | 'analysisId', value: string) => {
        setSelectedData((prev) => ({ ...prev, [type]: value }));
    };

    const handleFirstModalOk = async () => {
        try {
            setLoadingModal(true)
            console.log(stepInfo, 'selectedDataselectedDataselectedData');

            if (AIAnalysisResultDataRef.current && showPidRef.current) {
                const traceUrl = AIAnalysisResultDataRef.current[showPidRef.current].traceUrl;
                try {
                    let task1 = {
                        analysisId: selectedData.analysisId,
                        pids: [selectedData.pid],//需要是数组类型，数组中放1个pid
                        step_start: stepInfo.start[selectedData.stepBaseline.split('_')[1]],
                        step_end: stepInfo.start[selectedData.stepBaseline.split('_')[1]] + stepInfo.dur[selectedData.stepBaseline.split('_')[1]]
                    }

                    let task2 = {
                        analysisId: selectedData.analysisId,
                        pids: [selectedData.pid],//需要是数组类型，数组中放1个pid
                        step_start: stepInfo.start[selectedData.stepDiff.split('_')[1]],
                        step_end: stepInfo.start[selectedData.stepDiff.split('_')[1]] + stepInfo.dur[selectedData.stepDiff.split('_')[1]]
                    }
                    console.log("task:", task1, task2);
                    try {
                        let diffAnalysisRes = await GetAIDiffAnalysisResult(task1, task2);
                        // console.log(diffAnalysisRes)
                        const data: any = JSON.parse(JSON.parse(diffAnalysisRes.data)["data"]) as DiffTableRow[];
                        // console.log(data)
                        setDiffResultData(data);
                        setShowDiffModal(true);
                        setLoadingModal(false) //关闭model的loading
                    } catch (error) {
                        console.log(error);
                        setLoadingModal(false) //关闭model的loading

                    }
                    try {
                        await InitPerfetto({
                            callback: () => {
                                setTracingFlag(true);
                            }
                        }, "");
                    } catch (error) {
                        console.log('error', error);
                    }
                    OpenTraceFromUrl(traceUrl)
                } catch (error) {
                    console.error('Failed to get diff info:', error);
                    message.error('获取差分信息失败');
                }
            }
            setModalIsOpen(false);
            setLoadingModal(false) //关闭model的loading

        } catch (error) {
            console.error('Failed to get diff info:', error);
            message.error('获取差分信息失败');
            setLoadingModal(false) //关闭model的loading

        }
    };
    const closeModal = () => {
        setModalIsOpen(false)
    }
    const handleListRecord = async () => {
        try {
            const response: any = await GetListRecord({
                current: currentPage,
                pageSize: pageSize,
            });
            const data = response.data;
            setTotalRecord(response.total);
            if (response.code == "Success") {
                let datalist: any = [];
                data.forEach((item: any) => {
                    let record = {
                        analysisId: item.analysisId,
                        analysisTime: item.analysisTime,
                        instance: item.arguments.instance,
                        parms: [
                            { key: "timeout", value: item.arguments.timeout + " ms" },
                            { key: "analysisTool", value: item.arguments.analysisTool },
                        ],
                        failedLog: item.failedLog,
                        status: item.status
                    };
                    if (item.arguments.pids) {
                        record.parms.push({ key: "pids", value: item.arguments.pids });
                    }
                    if (item.arguments.comms) {
                        record.parms.push({ key: "comms", value: item.arguments.comms });
                    }

                    datalist.push(record);
                })
                setListRecordTableData(datalist);
            } else {
                const msg = "获取AI分析记录失败: " + response.message;
                message.error(msg);
            }
        } catch (error) {
            console.log('error', error);
        }
    };
    //判断数据（对象格式）中是否有所需要的 stepInfo 数据
    const hasStepInfo = (obj: any) =>
        Object.values(obj).some(value =>
            value !== null && typeof value === 'object' && Object.prototype.hasOwnProperty.call(value, 'stepInfo')
        );
    const handleAIInfraDiffAnalysis = async (v: any) => {
        if (!v["analysisId_1"]) {
            message.warning(intl.formatMessage({
                id: 'pages.ai_performance_diagnose.analysis_warn.analysis'
            }));
            return;
        }

        const now = new Date();
        try {
            const response1: any = await GetAIQueryResult(v["analysisId_1"])
            if (response1.code == "Success") {
                handleSelectChange('analysisId', v["analysisId_1"])
                const data: AiDataType = JSON.parse(response1.data);
                if (hasStepInfo(data)) {
                    AIAnalysisResultDataRef.current = data;

                    setAIAnalysisResultData(data);
                    const pids = Object.keys(AIAnalysisResultDataRef.current);
                    if (pids.length > 0) {
                        showPidRef.current = pids[0];
                        setShowPid(pids[0]);
                    } else {
                        setShowPid("暂无数据");
                    }
                    setModalIsOpen(true)
                    setPerfettoInit(perfettoInit + 1);
                } else {
                    message.error(`获取AI差分分析结果失败`);
                }

            } else {
                message.error(`获取AI差分分析结果失败`);
            }
        } catch (error) {
            console.log('error', error);
        }
    };

    return (
        <>
            <ProCard
                title={intl.formatMessage({
                    id: "pages.ai_performance_diagnose.analysis_parms",
                    defaultMessage: "Analysis Parms"
                })}
                bordered={false}
                style={{ maxWidth: '100%' }}
            >
                <ProForm
                    onFinish={handleAIInfraDiffAnalysis}
                >
                    <ProForm.Group>
                        {/* 输入实例ID */}
                        <ProFormText
                            width="md"
                            name="analysisId_1"
                            label={intl.formatMessage({
                                id: "pages.ai_performance_diagnose.diff_analysis_id_1",
                                defaultMessage: "Analysis ID 1"
                            })}
                            tooltip={
                                <>请输入已分析成功的分析ID</>
                            }
                            rules={[
                                {
                                    required: true,
                                    message: intl.formatMessage({
                                        id: "pages.ai_performance_diagnose.input_analysis",
                                    }),
                                },
                                {
                                    pattern: /^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$/,
                                    message: '请输入正确的分析ID'
                                }
                            ]}
                        />
                    </ProForm.Group>
                </ProForm>
            </ProCard>
            <Modal title="CPU/GPU Tracing对比分析"
                width="auto"
                // loading={!tracingFlag}
                open={modalIsOpen}
                // onOk={handleFirstModalOk}
                onCancel={closeModal}
                footer={null}
                destroyOnHidden={true}
            >
                <Spin spinning={loadingModal} tip="正在分析中..." size="large" >

                    <ProCard>
                        <ProForm
                            onFinish={handleFirstModalOk}
                        >
                            <ProForm.Group>
                                <Form.Item label="选择进程">
                                    <ProFormSelect
                                        options={
                                            AIAnalysisResultData
                                                ? Object.keys(AIAnalysisResultData).map((pid) => ({
                                                    value: pid,
                                                    label: pid,
                                                }))
                                                : []
                                        }
                                        rules={[
                                            {
                                                required: true,
                                                message: intl.formatMessage({
                                                    id: "pages.ai_performance_diagnose.input_analysis",
                                                }),
                                            },
                                        ]}
                                        onChange={(value: any) => handleSelectChange('pid', value)}
                                    />

                                </Form.Item>
                                <Form.Item label="选择基线 Step">
                                    <ProFormSelect
                                        options={
                                            stepInfo.id
                                        }
                                        rules={[
                                            {
                                                required: true,
                                                message: intl.formatMessage({
                                                    id: "pages.ai_performance_diagnose.input_analysis",
                                                }),
                                            },
                                        ]}
                                        onChange={(value: any) => handleSelectChange('stepBaseline', value)}
                                    />
                                </Form.Item>
                                <Form.Item label="选择对比 Step">
                                    <ProFormSelect
                                        options={
                                            stepInfo.id
                                        }
                                        rules={[
                                            {
                                                required: true,
                                                message: intl.formatMessage({
                                                    id: "pages.ai_performance_diagnose.input_analysis",
                                                }),
                                            },
                                        ]}
                                        onChange={(value: any) => handleSelectChange('stepDiff', value)}
                                    />
                                </Form.Item>
                            </ProForm.Group>
                        </ProForm>
                    </ProCard>

                    <ProCard
                        bordered={false}
                        style={{ maxWidth: '100%' }}
                    >
                        <div
                            className="chart-grid"
                            style={{
                                display: 'grid',
                                gridTemplateColumns: 'repeat(auto-fit, minmax(800px, 1fr))',
                                gap: '50px',
                            }}
                        >
                            {showEchart ?
                                <>
                                    <StepInfoBarChartNew stepInfo={stepInfo} />
                                    {stepInfo?.loss && stepInfo?.loss.legth > 0 && <StepLossBarChart stepInfo={stepInfo} />}
                                </> :
                                <div style={{ width: '100%', height: '100px', display: "flex", alignItems: 'center', justifyContent: 'center' }}>未采集到Step信息，建议使用Iteration模式触发AI Profiling</div>
                            }


                        </div>
                    </ProCard>
                </Spin>

            </Modal>
            <Modal
                title="性能差异分析结果"
                open={showDiffModal}
                onCancel={() => {
                    setShowDiffModal(false);
                    setModalIsOpen(false)
                    //   window.location.reload(); // 关闭时强制刷新页面
                }}
                footer={null}
                width={2000}
                destroyOnHidden={true}
            >
                <ProCard
                    bordered={false}
                    style={{ maxWidth: '100%' }}
                >
                    <div
                        className="chart-grid"
                        style={{
                            display: 'grid',
                            gridTemplateColumns: 'repeat(auto-fit, minmax(800px, 1fr))',
                            gap: '50px',
                        }}
                    >
                        {showEchart ?
                            <>
                                <StepInfoBarChartNew stepInfo={stepInfo} />
                                {stepInfo?.loss && stepInfo?.loss.legth > 0 && <StepLossBarChart stepInfo={stepInfo} />}
                            </> :
                            <div style={{ width: '100%', height: '100px', display: "flex", alignItems: 'center', justifyContent: 'center' }}>未采集到Step信息，建议使用Iteration模式触发AI Profiling</div>
                        }

                    </div>
                </ProCard>
                <DiffViewer
                    diffTable={diffResultData}
                    showDetails={true}
                />
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
            </Modal>
            <AiRecordTable
                dataSource={listRecordTableData}
                request={handleListRecord}
                setCurrentPage={setCurrentPage}
                setPageSize={setPageSize}
                total={totalRecord}
            />
        </>
    );
};
