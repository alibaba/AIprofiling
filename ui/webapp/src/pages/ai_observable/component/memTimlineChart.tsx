//import React, { useMemo } from 'react';
import React, { useState, useRef, useEffect, memo,useMemo } from 'react';
import {
  AreaChart, Area, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, Legend,
} from 'recharts';

interface MemoryProfilerProps {
  rawTimestamps: number[];
  rawValues: number[][];
}

// 索引 0 是 nouse，我们不再使用它
const INDEX_MAP = [
  "nouse",           // Index 0 (将被跳过)
  "PARAMETER",       // Index 6
  "OPTIMIZER_STATE", // Index 7
  "INPUT",           // Index 1
  "TEMPORARY",       // Index 2
  "ACTIVATION",      // Index 3
  "GRADIENT",        // Index 4
  "AUTOGRAD_DETAIL", // Index 5
  "unknown"          // Index 8
] as const;

// 渲染配置：移除了 nouse
const DISPLAY_CONFIG = [
  //{ key: "nouse", color: "#FFFFF", label: "NOUSE" },      // 绿 (最底层)
  { key: "PARAMETER", color: "#4C914C", label: "PARAMETER" },      // 绿 (最底层)
  { key: "OPTIMIZER_STATE", color: "#444444", label: "OPTIMIZER_STATE" }, // 黄
  { key: "INPUT", color: "#E9C46A", label: "INPUT" },              // 深灰
  { key: "TEMPORARY", color: "#A78BFA", label: "TEMPORARY" },      // 紫
  { key: "ACTIVATION", color: "#FF4D4D", label: "ACTIVATION" },    // 红
  { key: "GRADIENT", color: "#3B82F6", label: "GRADIENT" },        // 蓝
  { key: "AUTOGRAD_DETAIL", color: "#7DA1F4", label: "AUTOGRAD_DETAIL" }, // 浅蓝
  { key: "unknown", color: "#9CA3AF", label: "Unknown" },          // 灰
] as const;

const MemoryProfileChart: React.FC<MemoryProfilerProps> = ({ rawTimestamps, rawValues }) => {
  const chartData = useMemo(() => {
    if (!rawTimestamps?.length || !rawValues?.length) return [];
    
    const startTime = rawTimestamps[0];
    const step = Math.ceil(rawTimestamps.length / 2000); // 性能采样

    const result = [];
    for (let i = 0; i < rawTimestamps.length; i += step) {
      const ts = rawTimestamps[i];
      const vals = rawValues[i];
      const point: any = { 
        time: Math.floor((ts - startTime) / 1000) 
      };
      
      // 关键修改：从索引 1 开始循环，跳过 nouse
      INDEX_MAP.forEach((key, idx) => {
        if (idx === 0) return; // 彻底忽略 rawValues[i][0]
        point[key] = (vals[idx] || 0) / (1024 * 1024 * 1024); // 转为 GB
      });
      result.push(point);
    }
    return result;
  }, [rawTimestamps, rawValues]);

  // 计算最大分配内存
  const maxAllocated = useMemo(() => {
    const totals = chartData.map(d => 
      DISPLAY_CONFIG.reduce((sum, config) => sum + (d[config.key] || 0), 0)
    );
    return Math.max(...totals, 0).toFixed(2);
  }, [chartData]);

  return (
    <div style={{ width: '100%', height: '550px', background: '#fff' }}>
      <div style={{ textAlign: 'center', marginBottom: '10px' }}>
        <h2 style={{ fontSize: '18px', margin: '0' }}>Memory Usage Profile (Excluding nouse)</h2>
        <div style={{ fontSize: '14px', color: '#666' }}>Max allocated: {maxAllocated} GB</div>
      </div>

      <ResponsiveContainer width="100%" height="90%">
        <AreaChart data={chartData} margin={{ top: 10, right: 30, left: 20, bottom: 20 }}>
          <CartesianGrid strokeDasharray="3 3" vertical={false} stroke="#f0f0f0" />
          <XAxis 
            dataKey="time" 
            type="number" 
            domain={['dataMin', 'dataMax']} 
            tick={{fontSize: 12}}
            label={{ value: 'Time (ms)', position: 'insideBottom', offset: -10 }}
          />
          <YAxis 
            tick={{fontSize: 12}}
            label={{ value: 'Memory (GB)', angle: -90, position: 'insideLeft' }}
          />
          <Tooltip 
            formatter={(val: number) => val.toFixed(4) + " GB"}
            labelFormatter={(label) => `Time: ${label} ms`}
          />
          <Legend verticalAlign="top" align="right" />
          {DISPLAY_CONFIG.map((conf) => (
            <Area
              key={conf.key}
              type="linear"
              dataKey={conf.key}
              name={conf.label}
              stackId="1" // 保持相同的 stackId 确保紧密堆叠
              stroke={conf.color}
              fill={conf.color}
              fillOpacity={0.8}
              isAnimationActive={false}
              connectNulls
            />
          ))}
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
};

export default memo(MemoryProfileChart);
