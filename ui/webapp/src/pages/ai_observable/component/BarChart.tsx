/**
 * 此组件为双柱状图组件，支持显示柱状图每项显示组成，
 */
import React, { useState, useRef, useEffect, memo } from 'react';
import "./index.less";
import * as echarts from 'echarts';
import { useSize } from 'ahooks';
type EChartsOption = echarts.EChartsOption;

const BarChart = ({ size }: any) => {
    const chartRef = useRef<HTMLDivElement>(null); // 使用 useRef 来获取 DOM 元素
   
    useEffect(() => {
        var myChart = echarts.init(chartRef.current);;
        var option: EChartsOption;
        myChart.clear();
        const width1 = size!.width / 50;
        const width2 = size!.width / 150;
        const aaa: any = [
            {
                name: 'Email',
                type: 'bar',
                stack: 'a',
                barWidth: width1,
                emphasis: {
                    focus: 'series'
                },
                data: [120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210],
                Others: {
                    ComboCombined: []
                }
            },
            {
                name: 'Union Ads',
                type: 'bar',
                stack: 'a',
                barWidth: width1,

                emphasis: {
                    focus: 'series'
                },
                data: [220, 182, 191, 234, 290, 330, 310, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210],
                Others: {
                    ComboCombined: [
                        {
                            val: '1',
                            lab: 'a'

                        },
                        {
                            val: '2',
                            lab: 'b'
                        }
                    ]
                }
            },
            {
                name: 'Video Ads',
                type: 'bar',
                stack: 'a',
                barWidth: width1,

                emphasis: {
                    focus: 'series'
                },
                data: [150, 232, 201, 154, 190, 330, 410, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210],
                Others: {
                    ComboCombined: [
                        {
                            val: '1',
                            lab: 'a'

                        },
                        {
                            val: '2',
                            lab: 'b'
                        }
                    ]
                }
            },

            {
                name: 'Baidu',
                type: 'bar',
                stack: 'b',
                barWidth: width2,
                emphasis: {
                    focus: 'series'
                },
                data: ['', 732, 701, 734, 1090, 1130, 1120, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210],
                Others: {
                    ComboCombined: [
                        {
                            val: '1',
                            lab: 'a'

                        },
                        {
                            val: '2',
                            lab: 'b'
                        }
                    ]
                }
            },
            {
                name: 'Google',
                type: 'bar',
                stack: 'b',
                barWidth: width2,

                emphasis: {
                    focus: 'series'
                },
                data: ['', 132, 101, 134, 290, 230, 220, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210],
                Others: {
                    ComboCombined: [
                        {
                            val: '1',
                            lab: 'a'

                        },
                        {
                            val: '2',
                            lab: 'b'
                        }
                    ]
                }
            },
            {
                name: 'Bing',
                type: 'bar',
                stack: 'b',
                barWidth: width2,
                emphasis: {
                    focus: 'series'
                },
                data: ['', 72, 71, 74, 190, 130, 110, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210],
                Others: {
                    ComboCombined: [
                        {
                            val: '1',
                            lab: 'a'

                        },
                        {
                            val: '2',
                            lab: 'b'
                        }
                    ]
                }
            },
            {
                name: 'Others',
                type: 'bar',
                stack: 'b',
                barWidth: width2,
                emphasis: {
                    focus: 'series'
                },
                data: ['', 82, 91, 84, 109, 110, 120, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210, 120, 132, 101, 134, 90, 230, 210],
                Others: {
                    ComboCombined: [
                        {
                            val: '1',
                            lab: 'a'

                        },
                        {
                            val: '2',
                            lab: 'b'
                        }
                    ]
                }

            }
        ]
        option = {
            tooltip: {
                trigger: 'item',
                // axisPointer: {
                //     type: 'shadow'
                // },
                formatter: (params: any) => {
                    console.log(params);
                    let xsaa: any = ''
                    aaa.forEach((ite: any) => {
                        if (ite.name === params.seriesName) {
                            if (ite.Others.ComboCombined.length > 0) {
                                xsaa = `${params.seriesName} ${params.value} (${ite.Others.ComboCombined.map((item: any) => `${item.lab}:"${item.val}"`).join(',')})`
                            } else {
                                xsaa = `${params.seriesName} ${params.value}`
                            }
                        }
                    });
                    return xsaa;
                }
            },
            legend: {
                textStyle: {
                    color: '#000'
                }
            },
            grid: {
                left: '3%',
                right: '4%',
                bottom: '3%',
                containLabel: true
            },
            xAxis: {
                type: 'category',
                data: ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun', 'Mon1', 'Tue1', 'Wed1', 'Thu1', 'Fri1', 'Sat1', 'Sun1', 'Mon11', 'Tue11', 'Wed11', 'Thu11', 'Fri11', 'Sat11', 'Sun11', 'Mon12', 'Tue12', 'Wed12', 'Thu12', 'Fri12', 'Sat12', 'Sun12']
            },
            yAxis: {
                type: 'value'
            },
            series: aaa,
            //滚动条设置
            dataZoom: [{
                // 设置滚动条的隐藏与显示
                show: true,
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
                endValue: 15,
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
        return () => {
            if (myChart) {
                myChart.dispose(); // 组件卸载时销毁实例
            }
        };
    }, [size.width])


    return (

        <div ref={chartRef} id='pieChart' style={{ height: '500px' }}></div>

    );
};

export default memo(BarChart);