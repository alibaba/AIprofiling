import React, { useState, useRef, useEffect } from 'react';
import ProCard from '@ant-design/pro-card';
import { ProTable } from '@ant-design/pro-components';
import { Row, Col } from 'antd';
import { useIntl } from 'react-intl';
import * as echarts from 'echarts';
import { createPieOption } from './pieOption';
import "./index.less"
import { autoTimeConversion } from '@/utils/automaticUnitConversion';
import RingCharts from './RingCharts';
interface Config {
    cycle: boolean;
    select: boolean;
    selectData: string | null;
    outerRadius: number;
    innerRadius: number;
}

interface GPUSummaryBoardProps {
    summary: any;
    showpid: any;
}

export const GPUSummaryBoard: React.FC<GPUSummaryBoardProps> = (props) => {

    const intl = useIntl();
    const gpusummaryPieContainer = useRef(null);
    const labelMap: any = {
        "device_name": "设备名",
        "memory_size": "显存大小",
        "GPU_utilization": "GPU利用率",
        "active_blocks_per_SM": "每SM活跃blocks",
        "active_warps_per_SM": "每SM活跃warps",
        "SM_utilization": "SM利用率",
    };
    let execution_delay = props.summary?.execution_delay;
    let deviceInfo = props.summary?.device_info;
    let gpuUtilization = props.summary?.GPU_utilization;
    let totalDuration = execution_delay.total;
    let summaryTableData: any = [];
    let summaryPiedata: any = [];
    let summaryDeviceInfoData: any = [];
    let summaryGpuUtilizationData: any = [];
    let index = 0
    for (let item of deviceInfo) {
        const deviceInfokeys = Object.keys(item);
        summaryDeviceInfoData.push({
            label: "GPU" + index + " :  ",
            value: ""
        }, { label: "", value: "" });
        deviceInfokeys.forEach(key => {
            const cnlabel = labelMap[key];
            summaryDeviceInfoData.push({
                label: cnlabel + " :  ",
                value: item[key]
            })
        });
        index += 1;
    }
    const summaryGpuUtilizationkeys = Object.keys(gpuUtilization);
    summaryGpuUtilizationkeys.forEach(key => {
        const datakeys = Object.keys(gpuUtilization[key]);
        summaryGpuUtilizationData.push({
            label: key + " :  ",
            value: ""
        }, { label: "", value: "" });
        datakeys.forEach(label => {
            const cnlabel = labelMap[label];
            let value: string;
            if (typeof gpuUtilization[key][label] === "string") {
                value = gpuUtilization[key][label];
            } else {
                value = parseFloat(gpuUtilization[key][label].toFixed(2)).toString();
            }
            if (label == "GPU_utilization" || label == "SM_utilization") {
                value = value + "%"
            }
            summaryGpuUtilizationData.push({
                label: cnlabel + " :  ",
                value: value
            });
        });
    });
    const summaryDatakeys = Object.keys(execution_delay);
    summaryTableData.push({
        category: "total",
        duration: execution_delay["total"],
        percentage: 100
    });
    let otherTime = execution_delay["total"];
    let otherPercentage = 100;

    summaryDatakeys.forEach(key => {
        if (["cuda_runtime", "kernel", "cpu_op", "Kernel"].includes(key)) {
            let percentage = execution_delay[key] / totalDuration * 100;
            percentage = parseFloat(percentage.toFixed(2));
            summaryTableData.push({
                category: key,
                duration: execution_delay[key].toFixed(2),
                percentage: percentage
            });
            summaryPiedata.push(
                {
                    name: key,
                    value: execution_delay[key]
                });
            otherTime -= execution_delay[key];
            otherPercentage -= percentage;
        }
    });

    if (otherTime > 0) {
        summaryPiedata.push({
            name: "other",
            value: otherTime
        });

        summaryTableData.push({
            category: "other",
            duration: otherTime.toFixed(2),
            percentage: otherPercentage.toFixed(2)
        });
    }

    const summaryColumns = [
        { title: "种类", dataIndex: "category" },
        { title: "总时长", dataIndex: "duration", render: (val: any) => autoTimeConversion(val) },
        { title: "百分比(%)", dataIndex: "percentage" },
    ];
    useEffect(() => {
        if (gpusummaryPieContainer.current) {
            const gpuKernelTimePieOption = createPieOption(summaryPiedata);
            let gpuKernelTimePieChart = echarts.init(gpusummaryPieContainer.current);
            gpuKernelTimePieChart.setOption(gpuKernelTimePieOption);
        }
    }, [gpusummaryPieContainer, props.showpid]);
    const DiagnoseInfo = ({ diagnoseBasicInfo }: any) => {
        return (
            <Row
                wrap
                gutter={32}
                align="middle"
                style={{
                    height: 165,
                    overflowY: 'auto'
                }}
                children={
                    diagnoseBasicInfo &&
                    diagnoseBasicInfo.map((item: any) => (
                        <Col span="12">
                            <span
                                style={{
                                    color: '#000000',
                                    fontSize: '12px',
                                    lineHeight: '20px',
                                    fontWeight: 700,
                                }}
                            >
                                {item.label}
                            </span>
                            {item.value}
                        </Col>
                    ))
                }
            />
        );
    };
    return (
        <div className="app-container">
            <Row gutter={16} >
                <Col span={8} >
                    <div className='containerLeft'>
                        <div className='containerLeftTop'>
                            <ProCard
                                style={{ height: '100%' }}
                                title={intl.formatMessage({
                                    id: 'pages.ai_performance_diagnose.ai_infra.device_info',
                                    defaultMessage: "Device Info"
                                })}
                                bordered
                            >
                                <DiagnoseInfo
                                    diagnoseBasicInfo={summaryDeviceInfoData}
                                />
                            </ProCard>
                        </div>
                        <div className='containerLeftBottom'>
                            <ProCard
                                style={{ height: '100%' }}
                                title={intl.formatMessage({
                                    id: 'pages.ai_performance_diagnose.GPU_utilization',
                                    defaultMessage: "GPU Utilization"
                                })}
                                bordered
                            >
                                <DiagnoseInfo
                                    diagnoseBasicInfo={summaryGpuUtilizationData}
                                />
                            </ProCard>
                        </div>

                    </div>

                </Col>
                <Col span={16}>
                    <div className='containerRight'>
                        <ProCard
                            style={{ height: '100%' }}
                            title={intl.formatMessage({
                                id: 'pages.ai_performance_diagnose.execution_summary',
                                defaultMessage: "Execution Summary"
                            })}
                            bordered
                        >
                            <Row gutter={16} style={{ height: '100%', display: 'flex' }}>
                                <Col span={12} style={{ height: '100%', display: 'flex', flexDirection: 'column', justifyContent: 'center' }}>
                                    <div style={{ height: '100%', display: 'flex', flexDirection: 'column', justifyContent: 'center' }}>
                                        <ProTable
                                            headerTitle={"GPU内核函数详细信息统计表"}
                                            rowKey={'kernel_name'}
                                            columns={summaryColumns}
                                            dataSource={summaryTableData}
                                            pagination={{ defaultPageSize: 5 }}
                                            search={false}
                                        />
                                    </div>
                                </Col>
                                <Col span={12} style={{ height: '100%', display: 'flex', flexDirection: 'column' }}>
                                    {summaryPiedata && <RingCharts
                                        id={'gpusummaryPie'}
                                        data={summaryPiedata}
                                    />}
                                </Col>
                            </Row>
                        </ProCard>
                    </div>

                </Col>
            </Row>
        </div>
    );
};