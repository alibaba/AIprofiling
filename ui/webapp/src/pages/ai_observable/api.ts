// SPDX-License-Identifier: Apache-2.0
// AI observable REST API client.
import request from '../../utils/request';

type Options = Record<string, unknown> | undefined;

export interface StartAIAnalysisParams {
  instance: string;
  region?: string;
  iteration_mod?: string;
  iteration_func?: string;
  iteration_range?: [number, number];
  timeout?: number;
  pids?: string;
  comms?: string;
  channel?: string;
  uid?: string;
  instance_type?: string;
  analysis_params?: string[];
}

export async function StartAIAnalysis(params: StartAIAnalysisParams, options?: Options) {
  return request('/api/v1/app_observ/aiAnalysis/start_ai_analysis', {
    method: 'POST',
    data: {
      instance: params.instance,
      region: params.region,
      iteration_mod: params.iteration_mod,
      iteration_func: params.iteration_func,
      iteration_range: params.iteration_range,
      timeout: params.timeout,
      pids: params.pids,
      comms: params.comms,
      channel: params.channel,
      uid: params.uid,
      instance_type: params.instance_type,
      analysis_params: params.analysis_params,
    },
    ...(options || {}),
  });
}

export async function GetListRecord(params: Record<string, string | number>, options?: Options) {
  const queryParams = new URLSearchParams(
    Object.entries(params).reduce<Record<string, string>>((acc, [k, v]) => {
      acc[k] = String(v);
      return acc;
    }, {}),
  ).toString();
  return request(`/api/v1/app_observ/aiAnalysis/list_record?${queryParams}`, {
    method: 'GET',
    ...(options || {}),
  });
}

export async function GetAIQueryResult(analysisId: any, options?: Options) {
  return request('/api/v1/app_observ/aiAnalysis/query_result', {
    method: 'POST',
    data: { analysisId },
    ...(options || {}),
  });
}

export async function GetAIDiffAnalysisResult(taskOne: any, taskTwo: any, options?: Options) {
  return request('/api/v1/app_observ/aiAnalysis/diff_analysis', {
    method: 'POST',
    data: { task1: taskOne, task2: taskTwo },
    ...(options || {}),
  });
}

export async function ListProfilingClients(params?: Record<string, string | number>, options?: Options) {
  const queryParams = params
    ? `?${new URLSearchParams(
        Object.entries(params).reduce<Record<string, string>>((acc, [k, v]) => {
          acc[k] = String(v);
          return acc;
        }, {}),
      ).toString()}`
    : '';
  return request(`/api/clients${queryParams}`, {
    method: 'GET',
    ...(options || {}),
  });
}
