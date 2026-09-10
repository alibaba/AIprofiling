/**
 * 此组件为双柱状图组件，支持显示柱状图每项显示组成，
 */
import React, { useState, useRef, useEffect, memo } from 'react';
import "./index.less";
import * as echarts from 'echarts';
import { useSize } from 'ahooks';
import { autoTimeConversion } from '@/utils/automaticUnitConversion';
import { use } from 'echarts/types/src/extension.js';

const NewBarEcharts = ({ stepInfo, type }: any) => {
    const chartRef = useRef<HTMLDivElement>(null); // 使用 useRef 来获取 DOM 元素
    const [echartsData, setEchartsData] = useState<any>(null);
    const [rawData, setRawData] = useState<any>(null);
    const [isShow, setIsShow] = useState<boolean>(false);

    const size = useSize(chartRef);
    useEffect(() => {
        let seriesData: any = []
        let rawData: any = []
        if (type === 'step') {
            rawData = [
                { name: 'GPU运算用时', value: stepInfo?.computation || 0 },
                { name: 'GPU显存操作用时', value: stepInfo?.memory || 0 },
                { name: 'GPU通信用时', value: stepInfo?.communication || 0 },
                { name: 'GPU空闲时间', value: stepInfo?.other || 0 },
            ]
        } else if (type === 'noStep') {
            rawData = [{ name: 'loss', value: stepInfo?.loss || 0 }];
        }
        rawData.forEach((ite: any) => {
            if (ite.value != 0) {
                setIsShow(true)
            }
            seriesData.push({
                name: ite.name,
                data: ite.value,
                emphasis: {
                    focus: 'series'
                },
                type: 'bar',
                stack: 'time',
            })
        })
        setRawData(rawData)
        setEchartsData(seriesData)
    }, [stepInfo])
    useEffect(() => {
        if (chartRef.current && echartsData && isShow) {
            var myChart = echarts.init(chartRef.current);
            var option: any;
            myChart.clear();

            option = {
                tooltip: {
                    trigger: 'axis',
                    axisPointer: {
                        type: 'cross'
                    },
                    valueFormatter: function (value: number) {
                        return autoTimeConversion(Number(value))
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
                        data: stepInfo.id || []
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
                series: echartsData,
                //滚动条设置
                dataZoom: [{
                    // 设置滚动条的隐藏与显示
                    show: rawData[0]?.length > 10,
                    // 设置滚动条类型
                    type: "slider",
                    // 设置背景颜色
                    backgroundColor: "rgb(19, 63, 100)",
                    // 设置选中范围的填充颜色
                    fillerColor: "rgb(16, 171, 198)",
                    // 设置边框颜色
                    borderColor: "rgb(19, 63, 100)",
                    // 是否显示detail，即拖拽时候显示详细数值信息
                    showDetail: false,
                    // 数据窗口范围的起始数值
                    startValue: 0,
                    // 数据窗口范围的结束数值（一页显示多少条数据）
                    endValue: 10,
                    // empty：当前数据窗口外的数据，被设置为空。
                    // 即不会影响其他轴的数据范围
                    filterMode: "empty",
                    // 设置滚动条宽度，相对于盒子宽度
                    width: "90%",
                    // 设置滚动条高度
                    height: 10,
                    // 设置滚动条显示位置
                    left: "center",
                    // 是否锁定选择区域（或叫做数据窗口）的大小
                    zoomLock: true,
                    // 控制手柄的尺寸
                    handleSize: 0,
                    // dataZoom-slider组件离容器下侧的距离
                    bottom: 0,
                },
                {
                    // 没有下面这块的话，只能拖动滚动条，
                    // 鼠标滚轮在区域内不能控制外部滚动条
                    type: "inside",
                    // 滚轮是否触发缩放
                    zoomOnMouseWheel: false,
                    // 鼠标滚轮触发滚动
                    moveOnMouseMove: true,
                    moveOnMouseWheel: true,
                }]

            };

            option && myChart.setOption(option);
        }
        return () => {
            if (myChart) {
                myChart.dispose(); // 组件卸载时销毁实例
            }
        };
    }, [size, echartsData])


    return (

        <div ref={chartRef} id='pieChart' style={{ height: isShow ? '500px' : 0 }}></div>

    );
};

export default memo(NewBarEcharts);