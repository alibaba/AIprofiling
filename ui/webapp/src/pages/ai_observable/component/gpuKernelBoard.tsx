import React, { useState, useRef } from 'react';
import ProCard from '@ant-design/pro-card';
import { ProTable } from '@ant-design/pro-components';
import { Row, Col } from 'antd';
import '../result/index.less';
// import { FlamegraphRenderer, Box } from '@pyroscope/flamegraph';
import { Button, Modal, Popconfirm, Alert } from "antd";
import { FireOutlined } from '@ant-design/icons';
import { GetAIQueryResult } from '../api';
import { AiDataType } from './metaData';
import "./index.less"
import { autoTimeConversion, autoUnitConversion } from '@/utils/automaticUnitConversion';
import { useSize } from 'ahooks';
import RingCharts from './RingCharts';
const tableContainerStyle: React.CSSProperties = {
    paddingBottom: 10,
    width: "100%",
    height: "auto",
    borderBottom: '1px solidrgb(35, 35, 35)',
};
const sharedQuery = {
    searchQuery: "",
    onQueryChange: () => { },
    syncEnabled: true,
    toggleSync: () => { },
    id: "1"
}
interface GPUKernelBoardProps {
    detail: any;
    stackInfo: any;
    showpid: any;
    analysisId: any;
    totalTime: any;
}

export const GPUKernelBoard: React.FC<GPUKernelBoardProps> = (props) => {
    const refa = useRef(null);
    const size = useSize(refa);
    let kernel_details = props.detail?.kernel_details;
    let tensorCores_usage = props.detail?.tensorCores_usage;
    let kernel_stistics = props.detail?.kernel_stistics;

    const free_time = props.totalTime - (kernel_stistics?.Memory || 0) - (kernel_stistics?.Communication || 0) - (kernel_stistics?.Computation || 0);
    const kernelTableColumns: any = [];
    const kernelDetailskeys = Object.keys(kernel_details);
    const kernelSubDetailskeys = Object.keys(kernel_details).length > 0 ?
        Object.keys(kernel_details[kernelDetailskeys[0]]) :
        [];
    let ai_flamegraph = props.stackInfo;
    let totalTimePieData: any = [];
    let totalTimeTableData: any = [];
    const [modalKey, setModalKey] = useState(0);
    const [modalIsOpen, setModalIsOpen] = useState(false);
    const openModal = (funcName: any) => {
        sharedQuery.searchQuery = funcName
        setModalIsOpen(true)
    }

    const closeModal = () => {
        setModalKey(prevKey => prevKey + 1);
        setModalIsOpen(false)
    }
    const renderGPUKernelName = (value: any, record: any) => {

        return (
            <div className='tableToolip'>
                <Popconfirm
                    title={<div style={{ maxWidth: '550px', wordBreak: 'break-all', maxHeight: "250px", overflowY: 'auto' }}>{value}</div>}
                    showCancel={false}
                    trigger={['click', 'hover']}
                    placement="right"
                    overlayStyle={{ maxWidth: '600px', marginLeft: 20, marginRight: 20 }}
                >
                    <Button>
                        <div className='btnTooltip'> {value}</div>
                    </Button>
                </Popconfirm>

                {/* <Button
                    onClick={async () => {
                        const response: any = await GetAIQueryResult(props.analysisId);

                        if (response.code == "Success") {
                            const data: AiDataType = JSON.parse(response.data);
                            ai_flamegraph = data[props.showpid].stackInfo;
                        }
                        openModal(value);
                    }}
                    style={{ marginTop: 5 }}
                >
                    <FireOutlined style={{ color: 'red' }} />
                    点击查看AI火焰图
                </Button> */}

                {/* <Modal title="AI火焰图"
                    width="auto"
                    open={modalIsOpen}
                    onOk={closeModal}
                    onCancel={closeModal}>
                    <Box key={modalKey}>
                        {ai_flamegraph && Object.keys(ai_flamegraph).length > 0 ? (
                            <FlamegraphRenderer
                                profile={ai_flamegraph}
                                showToolbar={true}
                                colorMode='dark'
                                sharedQuery={sharedQuery}
                                onlyDisplay='both'
                            />) : (
                            <Alert
                                message="未获取到AI火焰图数据，请检查是否开启AI火焰图采集。若开启请稍后再试，通常需等待4～5分钟"
                                type="warning"
                                showIcon
                            />
                        )}
                    </Box>
                </Modal> */}
            </div>
        );
        
    };

    const kernelTableColumnsMap: any = {
        "kernel_name": { title: "GPU Kernel函数", align: 'center', dataIndex: "kernel_name", width: 190, fixed: 'left', render: renderGPUKernelName },
        "type_of_operation": { title: "算子类型", align: 'center', width: 100, dataIndex: "type_of_operation" },
        "run_times": {
            title: "运行次数",
            align: 'center',
            sorter: (a: any, b: any) => a.run_times - b.run_times,
            width: 100,
            dataIndex: "run_times"
        },
        "use_tensorCore": {
            title: "使用Tensor",
            align: 'center',
            width: 120,
            dataIndex: "use_tensorCore",
            sorter: (a: any, b: any) => a.use_tensorCore.localeCompare(b.use_tensorCore),
        },
        "total_delay_us": {
            title: "总时长",
            align: 'center',
            width: 130,
            dataIndex: "total_delay_us",
            sorter: (a: any, b: any) => a.total_delay_us - b.total_delay_us,
            render: (val: any) => autoTimeConversion(val)
        },
        "max_delay_us": {
            title: "最大时长",
            align: 'center',
            width: 130,
            dataIndex: "max_delay_us",
            sorter: (a: any, b: any) => a.max_delay_us - b.max_delay_us,
            render: (val: any) => autoTimeConversion(val),
        },
        "avg_delay_us": {
            title: "平均时长",
            align: 'center',
            width: 130,
            dataIndex: "avg_delay_us",
            sorter: (a: any, b: any) => a.avg_delay_us - b.avg_delay_us,
            render: (val: any) => autoTimeConversion(val)
        },
        "min_delay_us": {
            title: "最小时长",
            align: 'center',
            width: 130,
            dataIndex: "min_delay_us",
            sorter: (a: any, b: any) => a.min_delay_us - b.min_delay_us,
            render: (val: any) => autoTimeConversion(val)
        },
        "block": { title: "block", align: 'center', width: 120, dataIndex: "block" },
        "grid": { title: "grid", align: 'center', width: 120, dataIndex: "grid" },
        "shared_memory_size": {
            title: "共享内存大小",
            align: 'center',
            width: 140,
            dataIndex: "shared_memory_size",
            sorter: (a: any, b: any) => a.shared_memory_size - b.shared_memory_size,
            render: (val: any) => autoUnitConversion(val)
        },
        "SM_utilization": {
            title: "SM利用率(%)",
            align: 'center',
            width: 130,
            sorter: (a: any, b: any) => a.SM_utilization - b.SM_utilization,
            dataIndex: "SM_utilization"
        },
        "active_blocks_per_SM": {
            title: "每个SM上活跃block数",
            align: 'center',
            width: 180,
            sorter: (a: any, b: any) => a.active_blocks_per_SM - b.active_blocks_per_SM,
            dataIndex: "active_blocks_per_SM"
        },
        "active_warps_per_SM": {
            title: "每个SM上活跃warp数",
            align: 'center',
            width: 180,
            sorter: (a: any, b: any) => a.active_warps_per_SM - b.active_warps_per_SM,
            dataIndex: "active_warps_per_SM"
        },
        "registers_per_thread": {
            title: "每个线程使用的寄存器数量",
            align: 'center',
            width: 200,
            sorter: (a: any, b: any) => a.registers_per_thread - b.registers_per_thread,
            dataIndex: "registers_per_thread"
        },
    };

    kernelSubDetailskeys.forEach(val => {
        kernelTableColumns.push(kernelTableColumnsMap[val]);
    });

    kernelDetailskeys.forEach(key => {
        const delay = Number(kernel_details[key].total_delay_us);
        totalTimePieData.push({
            name: key,
            value: delay,
        });
        totalTimeTableData.push({
            kernel_name: key,
            type_of_operation: kernel_details[key].type_of_operation,
            run_times: kernel_details[key].run_times,
            use_tensorCore: kernel_details[key].use_tensorCore,
            total_delay_us: kernel_details[key].total_delay_us.toFixed(2),
            max_delay_us: kernel_details[key].max_delay_us.toFixed(2),
            avg_delay_us: kernel_details[key].avg_delay_us.toFixed(2),
            min_delay_us: kernel_details[key].min_delay_us.toFixed(2),
            block: kernel_details[key].block,
            grid: kernel_details[key].grid,
            SM_utilization: kernel_details[key].SM_utilization,
            active_blocks_per_SM: kernel_details[key].active_blocks_per_SM,
            active_warps_per_SM: kernel_details[key].active_warps_per_SM,
            registers_per_thread: kernel_details[key].registers_per_thread,
            shared_memory_size: kernel_details[key].shared_memory_size,
        })
    });

    return (
        <>

            <div className="bottom" ref={refa}>
                <Row gutter={[16, 16]} >
                    <Col xxl={8} xl={12} lg={24} sm={24} xs={24} >
                        <ProCard
                            title={"GPU 核函数类型统计图"}
                            bordered
                        >
                            <RingCharts
                                id={'tensorCoresPieContainer'}
                                data={[
                                    { value: kernel_stistics?.Computation , name: 'GPU运算用时' },
                                    { value: kernel_stistics?.Communication || 0, name: 'GPU通信用时' },
                                    { value: kernel_stistics?.Memory || 0, name: 'GPU显存操作时间' },
                                    { value: free_time || 0, name: 'GPU空闲时间' },
                                ]}
                                size={size || { width: 400, height: 400 }}
                            />
                        </ProCard>
                    </Col>
                    <Col xxl={8} xl={12} lg={24} sm={24} xs={24}>
                        <ProCard
                            title={"Tensor Cores 使用时间统计图"}
                            bordered
                        >
                            <RingCharts
                                id={'gpuKernelTensorC'}
                                data={[
                                    { value: tensorCores_usage?.service_time || 0, name: '使用时间' },
                                    { value: tensorCores_usage?.unused_time || 0, name: '未使用时间' }
                                ]}
                                size={size || { width: 400, height: 400 }}
                            />
                        </ProCard>
                    </Col>
                    <Col xxl={8} xl={24} lg={24} sm={24} xs={24}  >
                        <ProCard

                            title={"GPU内核函数调用时间统计图"}
                            bordered
                        >
                           {totalTimePieData && <RingCharts
                                id={'gpuKernelContainer'}
                                data={totalTimePieData}
                                size={size || { width: 400, height: 400 }}
                            />}
                        </ProCard>
                    </Col>
                </Row>
            </div>
            <div style={tableContainerStyle}  >
                <ProTable
                    headerTitle={"GPU内核函数详细信息统计表"}
                    rowKey={'kernel_name'}
                    columns={kernelTableColumns}
                    dataSource={totalTimeTableData}
                    search={false}
                    pagination={{ defaultPageSize: 5 }}
                    scroll={{ x: scrollX }}
                />
            </div>
        </>
    );
};
