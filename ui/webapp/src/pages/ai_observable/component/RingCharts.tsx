import React, { useState, useRef, useEffect, memo } from 'react';
import "./index.less";
import * as echarts from 'echarts';
import { useSize } from 'ahooks';
import { autoTimeConversion } from '@/utils/automaticUnitConversion';
import { get } from 'lodash';
type EChartsOption = echarts.EChartsOption;

const RingCharts = ({ size, data, id }: any) => {
    const chartRef = useRef<HTMLDivElement>(id); // 使用 useRef 来获取 DOM 元素
    let chartSize = useSize(chartRef);

    useEffect(() => {
        var myChart = echarts.init(chartRef.current);;
        var option: EChartsOption;
        myChart.clear()
        option = {
            legend: {
                type: 'scroll',
                top: 'center',
                left: '3%',
                orient: 'vertical',
                // with: 'auto',
                textStyle: {
                    color: '#000'
                },
                formatter: function (name: string) {
                    let names = ''
                    if (name == '使用时间' || name == '未使用时间') {
                        for (let i = 0; i < data.length; i++) {
                            if (data[i].name == name && data[i].value) {
                                names = name + ' : ' + autoTimeConversion(data[i].value)
                            }
                        }
                    } else {
                        let maxTextLength: number = 30; // 设置默认值

                        if (size && chartSize) {
                            if (size.height < 600) {
                                if (chartSize.width <= 600) {
                                    maxTextLength = 22;
                                } else if (chartSize.width > 600) {
                                    maxTextLength = 30;
                                }
                            } else if (size.height >= 600) {
                                if (chartSize.width > 1250) {
                                    maxTextLength = 130;
                                } else if (chartSize.width > 1070) {
                                    maxTextLength = 100;
                                } else if (chartSize.width > 800) {
                                    maxTextLength = 80;
                                }
                            }
                        }
                        // console.log(maxTextLength, 'maxTextLengthmaxTextLengthmaxTextLength');

                        if (name.length > maxTextLength) {
                            names = name.slice(0, maxTextLength - 3) + '...';
                        } else {
                            names = name
                        }
                    }
                    return names;
                }
            },
            tooltip: {
                trigger: 'item',
                confine: true,
                formatter: function (params: any) {

                    // 确保 params 存在且 name 是字符串
                    if (!params || !params.name || typeof params.name !== 'string') {
                        return ''; // 或者返回其他合理的默认值
                    }
                    const name = params.name;
                    const result = [];
                    for (let i = 0; i < name.length; i += 50) {
                        result.push(name.slice(i, i + 50)); // 每 50 个字符分为一组
                    }
                    // 拼接成正确的 HTML 结构
                    const str = result.map((item: string) => {
                        return `<div style="max-width: 500px;">${item}</div>`;
                    }).join('');

                    return `<div style="display: flex; align-items: center;">
                        <span style="display: inline-block; width: 10px; height: 10px; border-radius:50%; background-color: ${params.color};margin-right:5px;"></span>
                        <div>${str}</div>
                        <div style="margin:8px 0 8px 15px;">${autoTimeConversion(params.value)}</div>
                        <div style="margin-left:15px;">${params.percent}%</div>
                        </div>`;
                }
            },
            series: [
                {
                    type: 'pie',
                    radius: ['35%', '60%'],
                    center: chartSize && chartSize?.width > 752 ? ['80%', '50%'] : ['70%', '50%'], // 调整饼图的中心位置
                    avoidLabelOverlap: false,
                    itemStyle: {
                        borderRadius: 4,
                    },
                    label: {
                        show: false,
                        position: 'center',
                    },
                    emphasis: {
                        label: {
                            show: false,
                        }
                    },
                    labelLine: {
                        show: false
                    },
                    data: data
                }
            ]
        }
        option && myChart.setOption(option);
        return () => {
            if (myChart) {
                myChart.dispose(); // 组件卸载时销毁实例
            }
        };
    }, [data, id, chartSize])


    return (

        <div id={id} ref={chartRef} style={{ height: '500px', width: '100%' }}></div>

    );
};

export default memo(RingCharts);