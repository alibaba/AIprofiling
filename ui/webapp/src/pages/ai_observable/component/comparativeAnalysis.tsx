import { useEffect, useRef, useState } from 'react';
import { Col, Form, message, Modal, Row, Spin, Table, TableProps } from 'antd';
import { useIntl } from 'react-intl';
import { GetAIDiffAnalysisResult } from '../api'
import { DiffTableRow } from './metaData';
import { ProForm, ProFormSelect, ProCard } from '@ant-design/pro-components';
import { GPUTracing } from './perfettoTracing';
import { InitPerfetto, OpenTraceFromUrl } from 'aiprof-perfetto';
import { autoTimeConversion } from '@/utils/automaticUnitConversion';
import NewBarEcharts from './NewBarEcharts';
// import ComparisonFlamegraph from './ComparisonFlamegraph';

const TracingStyle: React.CSSProperties = {
    backgroundColor: 'white',
    color: 'black',
}
interface DiffViewerProps {
    diffTable: DiffTableRow[];
    showDetails?: boolean;
}

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

            render: (_, record: any) => (
                <div>
                    {showDetails && <div style={{ fontSize: '16px', color: record.time_diff && record.time_diff > 0 ? 'red' : (record.time_diff < 0 ? 'green' : 'white') }}>数值: {autoTimeConversion(record?.time_diff)}, 占比: {(record.time_perc_diff * 100).toFixed(2)}%</div>}
                </div>
            ),
        },
        {
            title: () => <span style={{ fontSize: '20px' }}>调用差异</span>,
            dataIndex: 'count_diff',
            key: 'count_diff',
            sorter: (a, b) => Number(a.count_diff - b.count_diff),
            render: (_, record: any) => (
                <div>
                    {showDetails && <div style={{ fontSize: '16px', color: record.count_diff && record.count_diff > 0 ? 'red' : (record.count_diff < 0 ? 'green' : 'white') }}>数值: {autoTimeConversion(record?.count_diff)}, 占比: {(record.count_perc_diff * 100).toFixed(2)}%</div>}
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
const ComparativeAnalysis = ({ resultData, pid, analysisId }: any) => {

    const intl = useIntl();
    const [stepInfo, setStepInfo] = useState<any>(null);
    const [AIAnalysisResultData, setAIAnalysisResultData] = useState<any>();
    const [showPid, setShowPid] = useState<string>('');
    const [showDiffModal, setShowDiffModal] = useState<boolean>(false);
    const [tracingFlag, setTracingFlag] = useState<boolean>(false);
    const [diffResultData, setDiffResultData] = useState([]);
    const [diffResultFlamegraph, setDiffResultFlamegraph] = useState([]);
    const [selectedData, setSelectedData] = useState<any>({});
    const [loadingModal, setLoadingModal] = useState<boolean>(false);

    useEffect(() => {
        getInitData(resultData, pid)
    }, [resultData, pid]);
    const getInitData = async (resultData: any, pid: string) => {
        setStepInfo(null)
        try {

            if (resultData && pid) {
                setAIAnalysisResultData(resultData);
                //获取包含 pid 的数据
                const result: any = Object.fromEntries(
                    Object.entries(resultData).filter(([key]) => key.startsWith(pid))
                );
                if (Object.keys(result).length > 0) {
                    //获取包含pid的key[]
                    const keys = Object.keys(result);
                    //获取第一个包含pid对应的value
                    const firstValue = result[keys[0]];
                    const stepInfoEnd: any = processStepData(firstValue.stepInfo);
                    setStepInfo(stepInfoEnd || null);
                    setShowPid(keys[0]);
                    handleSelectChange('pid', keys[0])
                } else {
                    message.error(`获取AI差分分析结果失败`);
                    setStepInfo(null)
                }

            } else {
                message.error(`获取AI差分分析结果失败`);
                setStepInfo(null);
            }
        } catch (error) {
            console.log('error', error);
        }

    }

    const processStepData = (steps: any[]) => {
        const id: string[] = [];
        const dur: number[] = [];
        const loss: number[] = [];
        const start: number[] = [];
        const memory: number[] = [];
        const communication: number[] = [];
        const computation: number[] = [];
        const other: number[] = [];
        // 累加 memory, computation 和 communication 值
        steps.forEach((step, index) => {
            id.push(`step_${index}`); // 生成唯一标识
            let duration = step.dur == 'N/A' ? 0 : step.dur;
            dur.push(step.dur == 'N/A' ? 0 : step.dur); // 提取 dur 值
            loss.push(step.loss == 'N/A' ? 0 : step.loss); // 提取 loss 值
            start.push(step.start == 'N/A' ? 0 : step.start); // 提取 dur 值
            let memorySum = 0;
            let communicationSum = 0;
            let computationSum = 0;
            for (const key in step) {
                if (key.startsWith('Memory:')) {
                    memorySum += step[key]; // 累加 memory:XX 的值
                } else if (key.startsWith('Communication:')) {
                    communicationSum += step[key]; // 累加 cuda_runtime:XX 的值
                } else if (key.startsWith('Computation:')) {
                    computationSum += step[key]; // 累加 cuda_runtime:XX 的值
                }
            }
            let value = (duration - memorySum - communicationSum - computationSum) < 0 ?
                0 : (duration - memorySum - communicationSum - computationSum).toFixed(2);
            other.push(Number(value))
            memory.push(memorySum < 0 ? 0 : memorySum); // 将 memory 总和存入数组
            communication.push(communicationSum < 0 ? 0 : communicationSum); // 将 communication 总和存入数组
            computation.push(computationSum < 0 ? 0 : computationSum); // 将 computation 总和存入数组
        });

        return { id, dur, loss, start, memory, communication, computation, other };
    };
    const handleSelectChange = (type: 'pid' | 'stepBaseline' | 'stepDiff' | 'analysisId', value: string) => {
        setSelectedData((prev: any) => ({ ...prev, [type]: value }));
    };
    const handleFirstModalOk = async () => {
        setLoadingModal(true)
        try {
            if (AIAnalysisResultData && showPid) {
                const traceUrl = AIAnalysisResultData[showPid].traceUrl;
                let task1 = {
                    analysisId: analysisId,
                    pids: [selectedData.pid],//需要是数组类型，数组中放1个pid
                    step_start: stepInfo.start[selectedData!.stepBaseline.split('_')[1]],
                    step_end: stepInfo.start[selectedData.stepBaseline.split('_')[1]] + stepInfo.dur[selectedData.stepBaseline.split('_')[1]]
                }
                let task2 = {
                    analysisId: analysisId,
                    pids: [selectedData.pid],//需要是数组类型，数组中放1个pid
                    step_start: stepInfo.start[selectedData.stepDiff.split('_')[1]],
                    step_end: stepInfo.start[selectedData.stepDiff.split('_')[1]] + stepInfo.dur[selectedData.stepDiff.split('_')[1]]
                }
                let diffAnalysisRes: any = await GetAIDiffAnalysisResult(task1, task2);
                if (diffAnalysisRes?.code !== "Success") {
                    message.error('获取差分信息失败');
                    setDiffResultData([])
                    setDiffResultFlamegraph([])
                    return
                }

                const data: any = JSON.parse(diffAnalysisRes.data["data"]) as DiffTableRow[];
                const flamegraph: any = JSON.parse(diffAnalysisRes.data["flamegraph"]);
                setDiffResultData(data);
                setDiffResultFlamegraph(flamegraph)
                setShowDiffModal(true);
                await InitPerfetto({
                    callback: () => {
                        setTracingFlag(true);
                    }
                }, "");
                OpenTraceFromUrl(traceUrl)
            }

        } catch (error) {
            console.error('Failed to get diff info:', error);
        } finally {
            setLoadingModal(false); // 确保加载状态最终关闭
        }
    };
    const checkArray = (arr: any) => {
        if (arr) {
            return !(arr.length === 0 || arr.every((item: number) => item == 0));
        } else {
            return false
        }
    }
    return (
        <div>
            <Spin spinning={loadingModal} tip="正在分析中..." size="large" >
                <ProForm
                    onFinish={handleFirstModalOk}
                >
                    <ProForm.Group>
                        {/* <Form.Item label="选择进程">
                            <ProFormSelect
                                options={
                                    AIAnalysisResultData ? Object.keys(AIAnalysisResultData).map((pid) => ({
                                        value: pid,
                                        label: pid,
                                    })) : []
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

                        </Form.Item> */}
                        <Form.Item label="选择基线 Step">
                            <ProFormSelect
                                options={
                                    stepInfo?.id
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
                                    stepInfo?.id
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
                {stepInfo ?
                    <Row gutter={[16, 16]} >
                        <Col xxl={checkArray(stepInfo?.loss) ? 12 : 24} xl={24} lg={24} sm={24} xs={24}>
                            <NewBarEcharts stepInfo={stepInfo} type={'step'} />
                        </Col>

                        {checkArray(stepInfo?.loss) && <Col xxl={12} xl={24} lg={24} sm={24} xs={24}>
                            <NewBarEcharts stepInfo={stepInfo} type={'noStep'} />
                        </Col>
                        }
                    </Row> :
                    <div style={{ width: '100%', height: '100px', display: "flex", alignItems: 'center', justifyContent: 'center' }}>未采集到Step信息，建议使用Iteration模式触发AI Profiling</div>
                }
            </Spin>
            <Modal
                title="性能差异分析结果"
                open={showDiffModal}
                onCancel={() => {
                    setShowDiffModal(false);
                    // setModalIsOpen(false)
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
                        {stepInfo ?
                            <Row gutter={[16, 16]} >
                                <Col xxl={checkArray(stepInfo?.loss) ? 12 : 24} xl={24} lg={24} sm={24} xs={24}>
                                    <NewBarEcharts stepInfo={stepInfo} type={'step'} />
                                </Col>

                                {checkArray(stepInfo?.loss) && <Col xxl={12} xl={24} lg={24} sm={24} xs={24}>
                                    <NewBarEcharts stepInfo={stepInfo} type={'noStep'} />
                                </Col>
                                }
                            </Row> :
                            <div style={{ width: '100%', height: '100px', display: "flex", alignItems: 'center', justifyContent: 'center' }}>未采集到Step信息，建议使用Iteration模式触发AI Profiling</div>
                        }

                    </div>
                </ProCard>
                <DiffViewer
                    diffTable={diffResultData}
                    showDetails={true}
                />
                {/* <div style={{ height: '1000px', overflowY: 'auto' }} >
                    <ComparisonFlamegraph flameGraphData={diffResultFlamegraph} />
                </div> */}

                <ProCard
                    key={"tracing"}
                    title={
                        <span >{"CPU/GPU Tracing分析"}</span>
                    }
                    loading={!tracingFlag}
                    bodyStyle={TracingStyle}
                >
                    <GPUTracing />
                </ProCard>
            </Modal>
        </div>
    )
};

export default ComparativeAnalysis
