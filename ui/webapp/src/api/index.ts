// SPDX-License-Identifier: Apache-2.0
import { withBase } from '../utils/basePath';

export type ApiEnvelope<T = any> = {
  code: 'Success' | 'Error' | 'NotImplemented' | string;
  message?: string;
  data?: T;
  total?: number;
};

async function request<T = any>(
  url: string,
  init?: RequestInit,
): Promise<ApiEnvelope<T>> {
  try {
    const resp = await fetch(withBase(url), {
      headers: { 'content-type': 'application/json' },
      ...init,
    });
    const text = await resp.text();
    let json: any;
    try {
      json = text ? JSON.parse(text) : {};
    } catch {
      return { code: 'Error', message: `Non-JSON response: ${text.slice(0, 120)}` };
    }
    if (!resp.ok && !json.code) {
      return { code: 'Error', message: `HTTP ${resp.status}` };
    }
    return json;
  } catch (err: any) {
    return { code: 'Error', message: err?.message ?? String(err) };
  }
}

export type StartAnalysisResult = {
  analysisId?: string;
  queued?: boolean;
  queuePosition?: number | null;
  queueFull?: boolean;
};

export function StartAIAnalysis(body: Record<string, any>) {
  return request('/api/v1/app_observ/aiAnalysis/start_ai_analysis', {
    method: 'POST',
    body: JSON.stringify(body),
  }) as Promise<ApiEnvelope<StartAnalysisResult> & StartAnalysisResult>;
}

// server 老列表格式 → 表格要的 { analysisId, analysisTime, instance, parms[], status, failedLog }
function normalizeListRow(row: any) {
  let args: any = {};
  try {
    args = typeof row.arguments === 'string' ? JSON.parse(row.arguments) : (row.arguments || {});
  } catch { /* keep {} */ }
  const parms: Array<{ key: string; value: any }> = [];
  const push = (k: string, v: any) => {
    if (v === undefined || v === null || v === '') return;
    if (Array.isArray(v) && v.length === 0) return;
    parms.push({ key: k, value: Array.isArray(v) ? v.join(' ') : v });
  };
  push('pids', args.pids);
  push('comms', args.comms);
  push('timeout', args.timeout);
  // analysis_params 数组 → 老前端约定的 "Params" key，AiRecordTable 会展成 analysis_params=...
  if (Array.isArray(args.analysis_params) && args.analysis_params.length) {
    parms.push({ key: 'Params', value: args.analysis_params.join(' ') });
  }
  return {
    analysisId: row.analysisId,
    analysisTime: row.analysisTime,
    instance: args.instance || row.instance || '',
    parms,
    status: row.status,             // server 已经返回中文，AiRecordTable 里已加中文映射
    failedLog: row.failedLog || '',
  };
}

export async function GetListRecord(params: { current?: number; pageSize?: number }) {
  const q = new URLSearchParams();
  Object.entries(params).forEach(([k, v]) => v !== undefined && q.set(k, String(v)));
  const resp = await request<any[]>(`/api/v1/app_observ/aiAnalysis/list_record?${q.toString()}`);
  if (resp.code === 'Success' && Array.isArray(resp.data)) {
    return { ...resp, data: resp.data.map(normalizeListRow) };
  }
  return resp;
}

export function GetAIQueryResult(analysisId: string) {
  return request('/api/v1/app_observ/aiAnalysis/query_result', {
    method: 'POST',
    body: JSON.stringify({ analysisId }),
  }).then((resp: any) => {
    // server 返回 { code, message, data: "<JSON string>" } — 这里预解，避免每个消费方各写一份
    if (resp && typeof resp.data === 'string') {
      try { resp.data = JSON.parse(resp.data); } catch { /* keep raw string */ }
    }
    return resp;
  });
}

export function GetAIDiffAnalysisResult(body: Record<string, any>) {
  return request('/api/v1/app_observ/aiAnalysis/diff_analysis', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function ListProfilingClients() {
  return request('/api/clients');
}

export function ListGpuProcs(clientId: string, timeoutMs = 3000) {
  return request(`/api/clients/${encodeURIComponent(clientId)}/gpu-procs?timeoutMs=${timeoutMs}`);
}

export function DeleteAnalysisRecord(analysisId: string) {
  return request(
    `/api/v1/app_observ/aiAnalysis/delete_record?analysisId=${encodeURIComponent(analysisId)}`,
    { method: 'DELETE' },
  );
}

export function GetExternalAnalyzer() {
  return request<{ url: string; importEnabled?: boolean }>('/api/settings/external-analyzer');
}

export type SessionUser = {
  userId: string;
  username: string;
  nickname: string;
  isAdmin: boolean;
  authMode: string;
  loginUrl?: string | null;
  logoutUrl?: string | null;
};

// 401 answers carry loginUrl at the envelope level, not under data.
export function GetSession() {
  return request<SessionUser>('/api/me') as Promise<
    ApiEnvelope<SessionUser> & { loginUrl?: string | null; authMode?: string }
  >;
}

