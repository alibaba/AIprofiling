import React, { useState, useRef, useEffect, memo } from 'react';
import "./index.less";
import * as echarts from 'echarts';
import { CanvasRenderer } from 'echarts/renderers'; // 导入 Canvas 渲染器
import { PieChart } from 'echarts/charts';
import {
    TitleComponent,
    TooltipComponent,
    LegendComponent
} from 'echarts/components';
import { use } from 'echarts/core';
import { useSize } from 'ahooks';

// 注册必须的模块
use([CanvasRenderer, PieChart, TitleComponent, TooltipComponent, LegendComponent]);

interface ReportLineChartType {
    title: string;
    id: string;
    chartData: {
        xdata: any[];
        data: any[];
        legend: any[];
    };
    height?: string;
    width?: number;
    padding?: string;
    radius?: string[];
    formatter?: string;
    options?: any;
    onclickPie?: (params: any) => void;
}
const ReportLineChart: React.FC<ReportLineChartType> = ({ title, chartData, id }) => {
    const { xdata, data, legend } = chartData
    const chartRef = useRef<HTMLDivElement>(null); // 使用 useRef 来获取 DOM 元素
    const size = useSize(chartRef);
    let myChart: any;
    useEffect(() => {
        if (data.length > 0 && chartRef.current && xdata.length > 0) {
            if (!myChart) {
                myChart = echarts.init(chartRef.current);
            }
            myChart.clear();
            var option: any;
            option = {
                backgroundColor: '#ffffff',
                width: '88%',
                tooltip: {
                    trigger: 'axis',
                    backgroundColor: 'rgba(255, 255, 255, 0.95)',
                    borderColor: '#ddd',
                    textStyle: {
                        color: '#333',
                        fontSize: 12
                    },
                    axisPointer: {
                        type: 'line',
                        lineStyle: {
                            color: '#aaa',
                            width: 1
                        }
                    }
                },
                legend: {
                    data: legend,
                    orient: 'horizontal',
                    left: 'center',
                    top: 10,
                    textStyle: {
                        color: '#333'
                    },
                    itemWidth: 12,
                    itemHeight: 2
                },
                dataZoom: [
                    {
                        type: 'slider',
                        show: true,
                        xAxisIndex: 0,
                        start: 0,
                        end: 100
                    },
                    {
                        type: 'inside',
                        xAxisIndex: 0,
                        start: 0,
                        end: 100,
                        zoomOnMouseWheel: true,
                        moveOnMouseMove: true,
                        preventDefaultMouseMove: true
                    }
                ],
                grid: {
                    left: '5%',
                    right: '5%',
                    bottom: '10%',
                    top: '15%',
                    containLabel: true
                },
                xAxis: {
                    name: '时间/ms',
                    nameTextStyle: {
                        color: '#666',
                        fontSize: 12
                    },
                    type: 'category',
                    data: xdata,
                    scale: true,
                    axisLine: {
                        lineStyle: {
                            color: '#ddd'
                        }
                    },
                    axisTick: {
                        lineStyle: {
                            color: '#ddd'
                        }
                    },
                    splitLine: {
                        show: true,
                        lineStyle: {
                            color: '#e8e8e8',
                            type: 'dashed'
                        }
                    },
                    axisLabel: {
                        color: '#666',
                        fontSize: 11
                    }
                },
                yAxis: {
                    name: 'Memory usage (Allocated) / GB',
                    nameLocation: 'middle',
                    nameGap: 40,
                    nameTextStyle: {
                        color: '#666',
                        fontSize: 12
                    },
                    type: 'value',
                    scale: true,
                    axisLine: {
                        lineStyle: {
                            color: '#ddd'
                        }
                    },
                    axisTick: {
                        lineStyle: {
                            color: '#ddd'
                        }
                    },
                    splitLine: {
                        show: true,
                        lineStyle: {
                            color: '#e8e8e8'
                        }
                    },
                    axisLabel: {
                        color: '#666',
                        fontSize: 11,
                        formatter: (value: number) => (value / (1024 ** 3)).toFixed(2) // 自动转为 GB 并保留两位小数
                    }
                },
                series: data.map((s: any) => {
                    // 创建一个新对象，排除 stack 属性
                    const { stack, ...rest } = s;
                    return {
                        ...rest,
                        type: 'line',
                        smooth: true,
                        symbol: 'circle',
                        symbolSize: 4,
                        lineStyle: {
                            width: 2
                        },
                        itemStyle: {
                            borderWidth: 2,
                            borderColor: '#fff',
                            opacity: 0.9
                        }
                    };
                })
            };
            option && myChart.setOption(option);
        }
        return () => {
            if (myChart) {
                myChart.dispose(); // 组件卸载时销毁实例
            }
        };
    }, [chartData, size?.width]);
    return (
        <div className='reportLineChart' style={{height:xdata.length > 0? '400px':'150px'}}>
            {title && <div className='reportLineTitle'>{title}</div>}
            {data.length > 0 && xdata.length > 0 ?
                <div ref={chartRef} id={id} style={{ height: "375px", width: '100%' }}></div>
                : <div className='noDataTip' style={{height:'100px'}}>暂无数据</div>
            }
        </div>

    );
};

export default memo(ReportLineChart);