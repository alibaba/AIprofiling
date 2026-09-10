// SPDX-License-Identifier: Apache-2.0
import React, { useMemo } from 'react';
import ReactECharts from 'echarts-for-react';

export type RingItem = { name: string; value: number };

interface Props {
  title: string;
  items: RingItem[];
  colors?: string[];
  height?: number;
  unit?: string;
}

const DEFAULT_COLORS = [
  '#1677ff', '#5b8ff9', '#69b1ff', '#61ddaa',
  '#65789b', '#f6bd16', '#e86452', '#9270ca',
  '#ff9d4d', '#269a99',
];

export const RingChart: React.FC<Props> = ({ title, items, colors = DEFAULT_COLORS, height = 260, unit = '' }) => {
  const option = useMemo(() => {
    const total = items.reduce((s, it) => s + it.value, 0) || 1;
    return {
      title: {
        text: title,
        left: 'center',
        top: 4,
        textStyle: { fontSize: 13, fontWeight: 500, color: '#6b7280' },
      },
      tooltip: {
        trigger: 'item',
        formatter: (p: any) =>
          `${p.marker} ${p.name}<br/>${p.value}${unit ? ' ' + unit : ''} (${((p.value / total) * 100).toFixed(1)}%)`,
      },
      legend: {
        orient: 'vertical',
        right: 8,
        top: 'middle',
        itemGap: 6,
        textStyle: { fontSize: 12, color: '#4b5563' },
        formatter: (name: string) => (name.length > 22 ? name.slice(0, 22) + '…' : name),
      },
      color: colors,
      series: [
        {
          name: title,
          type: 'pie',
          radius: ['48%', '68%'],
          center: ['35%', '52%'],
          avoidLabelOverlap: true,
          label: { show: false },
          labelLine: { show: false },
          data: items.map((it) => ({ name: it.name, value: it.value })),
        },
      ],
    };
  }, [title, items, colors, unit]);

  return <ReactECharts option={option} style={{ height, width: '100%' }} />;
};

export default RingChart;
