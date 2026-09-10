import ProCard from '@ant-design/pro-card';
import {
    ProForm,
    ProFormText,
    ProFormSelect,
    ProFormCascader,
    ProFormCheckbox,
    ProFormDigitRange,
    ProFormDigit,
    ProFormDependency
} from '@ant-design/pro-components';
import { message, Row, Col } from 'antd';
import React, { useState, useEffect } from 'react';
import { useIntl } from 'react-intl';
import { StartAIAnalysis, GetListRecord } from './api'
import { AiRecordTable } from './component/aiRecordTable'


const analysisParsmsMap:any = {
    "gpu"      : "GPU算子 ",
    "cpu"      : "CPU信息 ",
    "stack"    : "Python调用栈 ",
    "memory"   : "Torch显存 ",
    "adapt"    : '自适应采集指标',
    "kernel"   : 'GPU Kernel',
    "pytorch"  : 'PyTorch',
    "python"   : 'Python Stack',
    "snapshot" : 'GPU显存快照',
}

const MAX_ANALYSIS_TIME = 60000;
const MIN_ANALYSIS_TIME = 1000;

export const EcsChannelPage = () => {
    const intl = useIntl();
    const [totalRecord, setTotalRecord] = useState<number>(0);
    const [listRecordTableData, setListRecordTableData] = useState([]);
    const [currentPage, setCurrentPage] = useState<number>(1);
    const [pageSize, setPageSize] = useState<number>(10);

    const handleListRecord = async () => {
        const verifyTime = (time: string) => {
            const now = new Date();
            const date = new Date(time.replace(/ /, 'T'));
            const timeDifference = Math.abs(date.getTime() - now.getTime());

            return timeDifference >= 10 * 60 * 1000 + 2000;
        }

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
                    console.log("====record", item)
                    let record = {
                        analysisId: item.analysisId,
                        analysisTime: item.analysisTime,
                        instance: item.arguments.instance,
                        parms: [
                            { key: "channel", value: item.arguments.channel? item.arguments.channel : "离线通道" },
                        ],
                        failedLog: item.failedLog,
                        status: item.status
                    };
                    if (verifyTime(record.analysisTime) && (record.status == "采集中" || record.status == "分析中")) {
                        record.status = "采集失败";
                        record.failedLog = "采集超时, 请检查进程是否占用GPU或是否为python进程";
                    }
                    if (item.arguments.pids) {
                        record.parms.push({ key: "pids", value: item.arguments.pids });
                    }
                    if (item.arguments.comms) {
                        record.parms.push({ key: "comms", value: item.arguments.comms });
                    }
                    if (item.arguments.timeout) {
                        record.parms.push({ key: "timeout", value: item.arguments.timeout + " ms" });
                    } else if (item.arguments.iteration_range) {
                        record.parms.push({ key: "iteration", value: item.arguments.iteration_range[1] - item.arguments.iteration_range[0] });
                    }
                    let params_label = "";
                    for (const key in item.arguments.analysis_params) {
                        params_label += analysisParsmsMap[item.arguments.analysis_params[key]];
                    }
                    record.parms.push({ key: "Params", value: params_label });

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
    const handleAIInfraAnalysis = async (v: any) => {
        // if (!v["instance"] || (!v["analysisTimeout"] && !v["analysisMode"])) {
        //     message.warning(intl.formatMessage({
        //         id: 'pages.ai_performance_diagnose.analysis_warn.time_instanceID_iteration'
        //     }));
        //     return;
        // }

        // if (!v["pids"] && !v["comms"]) {
        //     message.warning(intl.formatMessage({
        //         id: 'pages.ai_performance_diagnose.analysis_warn.pid_comm'
        //     }));
        //     return;
        // }

        const time = Number(v["analysisTimeout"])
        // if (!v["analysisTimeout"] && typeof time !== "number") {
            if (typeof time !== "number" || time < MIN_ANALYSIS_TIME || time > MAX_ANALYSIS_TIME) {
            message.warning(intl.formatMessage({
                id: 'pages.ai_performance_diagnose.analysis_warn.time'
            }));
            return;
        }

        const now = new Date();
        const formattedDate = now.toISOString().replace('T', ' ').substring(0, 19);
        try {
            setListRecordTableData((prevData: any) => [
                {
                    analysisId: "分析ID生成中...",
                    instance: v["instance"],
                    analysisTime: formattedDate,
                    parms: [
                        { key: 'timeout', value: v["analysisTimeout"] + " ms" },
                        { key: 'pids', value: v["pids"] }
                    ],
                    failedLog: "",
                    status: "采集中"
                },
                ...prevData,
            ]);
            const response: any = await StartAIAnalysis({
                instance: v["instance"],
                region: v["region"][1],
                timeout: v["analysisTimeout"],
                iteration_mod: v["iterationModule"] ? v["iterationModule"] : "",
                iteration_func: v["iterationFunc"] ? v["iterationFunc"] : "",
                iteration_range: v["iterationRange"],
                analysis_params: v["analysis_params"],
                pids: v["pids"],
                comms: v["comms"],
                channel: "ecs_console",
                uid: v["uid"],
            });
            if (response.code == "Success") {
                message.success("AI Profiling触发成功");
            } else {
                const msg = "AI Profiling触发失败: " + response.message;
                message.error(msg);
            }
            handleListRecord();
        } catch (error) {
            console.log('error', error);
        }
    };
    // interface1: 获取实例id列表数据
    useEffect(() => {
        handleListRecord();
        const intervalId = setInterval(handleListRecord, 10000);

        return () => clearInterval(intervalId);
    }, [currentPage, pageSize]);
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
                    onFinish={handleAIInfraAnalysis}
                >
                    <ProForm.Group>
                        {/* 输入实例ID */}
                        <ProFormText
                            width="md"
                            name="instance"
                            label={intl.formatMessage({
                                id: "pages.ai_performance_diagnose.instanceId",
                                defaultMessage: "Instance ID"
                            })}
                            placeholder={intl.formatMessage({
                                id: "pages.ai_performance_diagnose.select_instanceId",
                            })}
                            // rules={[
                            //     {
                            //         required: true,
                            //         message: intl.formatMessage({
                            //             id: "pages.ai_performance_diagnose.select_instanceId",
                            //         }),
                            //     },
                            // ]}
                        />
                        <ProFormCascader
                            width="md"
                            name="region"
                            label={intl.formatMessage({
                                id: "pages.ai_performance_diagnose.region",
                                defaultMessage: "Instance Region"
                            })}
                            request={async () => [
                                {
                                    label: '华东',
                                    value: 'huadong',
                                    children: [
                                        {
                                            value: 'cn-hangzhou',
                                            label: '华东1（杭州）',
                                        },
                                        {
                                            value: 'cn-shanghai',
                                            label: '华东2（上海）',
                                        },
                                        {
                                            value: 'cn-nanjing',
                                            label: '华东5（南京-本地地域）',
                                        },
                                        {
                                            value: 'cn-fuzhou',
                                            label: '华东6（福州-本地地域）',
                                        },
                                    ],
                                },
                                {
                                    label: '华北',
                                    value: 'huabei',
                                    children: [
                                        {
                                            value: 'cn-qingdao',
                                            label: '华北1（青岛）',
                                        },
                                        {
                                            value: 'cn-beijing',
                                            label: '华北2（北京）',
                                        },
                                        {
                                            value: 'cn-zhangjiakou',
                                            label: '华北3（张家口）',
                                        },
                                        {
                                            value: 'cn-huhehaote',
                                            label: '华北5（呼和浩特）',
                                        },
                                        {
                                            value: 'cn-wulanchabu',
                                            label: '华北6（乌兰察布）',
                                        },
                                    ],
                                },
                                {
                                    label: '华南',
                                    value: 'huanan',
                                    children: [
                                        {
                                            value: 'cn-shenzhen',
                                            label: '华南1（深圳）',
                                        },
                                        {
                                            value: 'cn-heyuan',
                                            label: '华南2（河源）',
                                        },
                                        {
                                            value: 'cn-guangzhou',
                                            label: '华南3（广州）',
                                        },
                                    ],
                                },
                                {
                                    label: '西南',
                                    value: 'xinan',
                                    children: [
                                        {
                                            value: 'cn-chengdu',
                                            label: '西南1（成都）',
                                        }
                                    ],
                                },
                                {
                                    label: '华中',
                                    value: 'huazhong',
                                    children: [
                                        {
                                            value: 'cn-wuhan',
                                            label: '华中1（武汉-本地地域）',
                                        }
                                    ],
                                },
                                {
                                    label: '中国（香港）',
                                    value: 'cn-hongkong'
                                },
                            ]}
                            rules={[
                                {
                                    required: true,
                                    message: intl.formatMessage({
                                        id: "pages.ai_performance_diagnose.input_region",
                                    }),
                                },
                            ]}
                        />
                        {/* BUC ID */}
                        <ProFormText
                            width="md"
                            name="uid"
                            label={intl.formatMessage({
                                id: "pages.ai_performance_diagnose.uid",
                                defaultMessage: "UID"
                            })}
                            placeholder={intl.formatMessage({
                                id: 'pages.ai_performance_diagnose.input_uid',
                                defaultMessage: "input ai task pid"
                            })}
                            tooltip="填写目标实例的 UID（如未开通操作系统控制台，请先在控制台完成实例接入）"
                            rules={[
                                {
                                    required: true,
                                    message: intl.formatMessage({
                                        id: "pages.ai_performance_diagnose.input_uid",
                                    }),
                                },
                            ]}
                        />
                        {/* 输入AI作业PID */}
                        <ProFormText
                            width="md"
                            name="pids"
                            label={intl.formatMessage({
                                id: "pages.ai_performance_diagnose.ai_task_pid",
                                defaultMessage: "ai task pid"
                            })}
                            placeholder={intl.formatMessage({
                                id: 'pages.ai_performance_diagnose.input_ai_task_pid',
                                defaultMessage: "input ai task pid"
                            })}
                            // rules={[
                            //     {
                            //         message: intl.formatMessage({
                            //             id: "pages.ai_performance_diagnose.input_ai_task_pid_error",
                            //         }),
                            //         type: 'string',
                            //         pattern: /^(\d+,)*\d+$/,
                            //     },
                            // ]}
                        />
                        {/* 输入AI作业进程名 */}
                        <ProFormText
                            width="md"
                            name="comms"
                            label={intl.formatMessage({
                                id: "pages.ai_performance_diagnose.ai_task_process_name",
                                defaultMessage: "ai_task_pid"
                            })}
                            placeholder={intl.formatMessage({
                                id: 'pages.ai_performance_diagnose.input_ai_task_process',
                                defaultMessage: "input ai task process name"
                            })}
                            // rules={[
                            //     {
                            //         message: intl.formatMessage({
                            //             id: "pages.ai_performance_diagnose.input_ai_task_process",
                            //         }),
                            //         type: 'string',
                            //     },
                            // ]}
                        />
                    </ProForm.Group>

                    <ProForm.Group>
                        {/* 输入数据丰富度 */}
                        <ProFormSelect
                            name="analysis_params"
                            width="md"
                            label={intl.formatMessage({
                                id: "pages.ai_performance_diagnose.ai_task_analysis_tool",
                                defaultMessage: "AI Analysis Params"
                            })}
                            valueEnum={{
                                adapt    : '自适应采集指标',
                                kernel   : 'GPU Kernel',
                                pytorch  : 'PyTorch',
                                python   : 'Python Stack',
                                snapshot : 'GPU显存快照',
                            }}
                            initialValue={['adapt']}
                            fieldProps={{
                                mode: 'multiple',
                            }}
                            rules={[
                                {
                                    required: true,
                                    message: '请选择合适的数据丰富度',
                                    type: 'array',
                                },
                            ]}
                            tooltip="AI Profiling 数据丰富度：选择需要采集的指标类别（自适应 / Kernel / PyTorch / Python 栈 / 显存快照）。如需 NVTX / NCCL / RDMA / DCGM / ROCm 等更丰富指标，请到阿里云操作系统控制台使用"
                        />
                        <ProFormSelect
                            options={[
                                {
                                    value: 'duration',
                                    label: 'duration模式',
                                },
                                {
                                    value: 'iteration',
                                    label: 'iteration模式',
                                }
                            ]}
                            initialValue={{
                                value: 'duration',
                                label: 'duration模式',
                            }}
                            width="md"
                            name="analysisMode"
                            label={intl.formatMessage({
                                id: "pages.ai_performance_diagnose.ai_task_analysis_mode",
                                defaultMessage: "ai task analysis mode"
                            })}
                        />
                        <ProFormDependency
                            name={['analysisMode']}
                        >
                            {({ analysisMode }) => {
                                if (analysisMode === 'iteration') {
                                    return (
                                        <Row gutter={16}>
                                            <Col span={8}>
                                                <ProFormDigitRange
                                                    label={intl.formatMessage({
                                                        id: "pages.ai_performance_diagnose.analysis_iteration",
                                                        defaultMessage: "Analysis Iteration Range"
                                                    })}
                                                    name="iterationRange"
                                                    separator="-"
                                                    placeholder={['起始采集迭代数', '终止采集迭代数']}
                                                    initialValue={[0, 10]}
                                                    tooltip={"迭代数:指数据采集模块激活时的迭代次数，独立于AI作业的迭代计数。"}
                                                />
                                            </Col>
                                            <Col span={8}>
                                                <ProFormText
                                                    width="md"
                                                    name="iterationModule"
                                                    label={intl.formatMessage({
                                                        id: "pages.ai_performance_diagnose.analysis_iteration_mod",
                                                        defaultMessage: "Analysis Iteration Module"
                                                    })}
                                                    placeholder={"请输入迭代的入口模块(a.b.module)"}
                                                    tooltip={"请输入迭代的入口模块"}
                                                />
                                            </Col>
                                            <Col span={8}>
                                                <ProFormText
                                                    width="md"
                                                    name="iterationFunc"
                                                    label={intl.formatMessage({
                                                        id: "pages.ai_performance_diagnose.analysis_iteration_func",
                                                        defaultMessage: "Analysis Iteration Function"
                                                    })}
                                                    placeholder={"请输入迭代的入口函数(Class.function)"}
                                                    tooltip={"请输入迭代的入口函数。vllm推理场景默认为:vllm.worker.work_xxx.yyyWorker.execute_model;训练场景默认为:Optimizerstep"}
                                                />
                                            </Col>
                                        </Row>
                                    );
                                }
                                return (
                                    <ProFormDigit
                                        width="md"
                                        name="analysisTimeout"
                                        label={intl.formatMessage({
                                            id: "pages.ai_performance_diagnose.analysis_time",
                                            defaultMessage: "analysis_time"
                                        })}
                                        placeholder={intl.formatMessage({
                                            id: 'pages.ai_performance_diagnose.input_analysis_time',
                                            defaultMessage: "Input Analysis Time"
                                        })}
                                        initialValue={2000}
                                    />
                                );
                            }}
                        </ProFormDependency>
                    </ProForm.Group>
                </ProForm>
            </ProCard>
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