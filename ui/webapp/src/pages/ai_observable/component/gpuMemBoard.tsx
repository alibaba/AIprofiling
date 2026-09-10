// GPUMemBoard.tsx
import React from 'react';
import type { ProColumns } from '@ant-design/pro-components';
import { ProTable } from '@ant-design/pro-components';
import type { GpuMemAllocSpikes } from './metaData';
interface GPUMemBoardProps {
  data: GpuMemAllocSpikes[];
  pageSize?: number; // 默认 5
}
const GPUMemBoard: React.FC<GPUMemBoardProps> = ({
  data,
  pageSize = 5,
}) => {
  const columns: ProColumns<GpuMemAllocSpikes>[] = [
    {
    
      title: '地址',
      dataIndex: 'addr',
      width: '13.333%',
      sorter: (a, b) => a.size - b.size,
    },
    {
      title: '时间',
      dataIndex: 'time',
      width: '17.333%',
      sorter: (a, b) => a.time.localeCompare(b.time),
      // sorter: (a, b) => a.time.localeCompare(b.time),
    },
    {
      title: '大小',
      dataIndex: 'size',
      valueType: 'digit',
      width: '9.777%',
      sorter: (a, b) => a.size - b.size,
    },
    {
      title: '堆栈',
      dataIndex: 'stack',
      ellipsis: true,
      width: '60.000%'
    },
  ];
  return (
    <ProTable<GpuMemAllocSpikes>
      rowKey={(record, index) => `${record.time}-${index}`}
      columns={columns}
      // 静态数据直接通过 dataSource 传入
      dataSource={data}
      search={false}               // 不展示搜索表单
      options={false}              // 隐藏右上角设置、刷新等按钮
      pagination={{
        pageSize,
        showSizeChanger: false,    // 不允许用户改 pageSize（如果你需要可以设为 true）
        showTotal: (total) => `共 ${total} 条`,
      }}
      toolBarRender={false}        // 隐藏工具栏
    />
  );
};
export default GPUMemBoard;