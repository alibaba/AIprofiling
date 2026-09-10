// SPDX-License-Identifier: Apache-2.0
// AI records list page.
// 触发采集入口在 ecsChannel/starAgentChannel，本文件仅提供列表能力。
import React, { useState, useEffect } from 'react';
import { message } from 'antd';
import { GetListRecord } from './api';
import { AiRecordTable } from './component/aiRecordTable';

const analysisParsmsMap: Record<string, string> = {
  gpu: 'GPU算子 ',
  cpu: 'CPU信息 ',
  stack: 'Python调用栈 ',
  memory: 'Torch显存 ',
};

export const ListRecordPage: React.FC = () => {
  const [totalRecord, setTotalRecord] = useState<number>(0);
  const [listRecordTableData, setListRecordTableData] = useState<any[]>([]);
  const [currentPage, setCurrentPage] = useState<number>(1);
  const [pageSize, setPageSize] = useState<number>(10);

  const handleListRecord = async () => {
    try {
      const response: any = await GetListRecord({
        current: currentPage,
        pageSize,
      });
      const data = response?.data ?? [];
      setTotalRecord(response?.total ?? 0);
      if (response?.code === 'Success') {
        const datalist: any[] = [];
        data.forEach((item: any) => {
          let args: any = {};
          try {
            args = JSON.parse(item.arguments ?? '{}');
          } catch {
            args = {};
          }
          const record: any = {
            analysisId: item.analysisId,
            analysisTime: item.analysisTime,
            instance: args.instance,
            parms: [{ key: 'channel', value: args.channel ? args.channel : '离线通道' }],
            failedLog: item.failedLog,
            status: item.status,
            hasReport: item.hasReport,
          };
          if (args.pids) record.parms.push({ key: 'pids', value: args.pids });
          if (args.comms) record.parms.push({ key: 'comms', value: args.comms });
          if (args.timeout) {
            record.parms.push({ key: 'timeout', value: args.timeout + ' ms' });
          } else if (args.iteration_range) {
            record.parms.push({
              key: 'iteration',
              value: args.iteration_range[1] - args.iteration_range[0],
            });
          }
          let paramsLabel = '';
          if (args.analysis_params) {
            for (const k of Object.keys(args.analysis_params)) {
              paramsLabel += analysisParsmsMap[args.analysis_params[k]] ?? '';
            }
          }
          if (paramsLabel.trim()) {
            record.parms.push({ key: 'Params', value: paramsLabel });
          }
          datalist.push(record);
        });
        setListRecordTableData(datalist);
      } else if (response?.message) {
        message.error('获取AI分析记录失败: ' + response.message);
      }
    } catch (error) {
      console.log('error', error);
    }
  };

  useEffect(() => {
    handleListRecord();
    const intervalId = setInterval(handleListRecord, 10000);
    return () => clearInterval(intervalId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentPage, pageSize]);

  return (
    <AiRecordTable
      dataSource={listRecordTableData}
      request={handleListRecord}
      setCurrentPage={setCurrentPage}
      setPageSize={setPageSize}
      total={totalRecord}
    />
  );
};

export default ListRecordPage;
