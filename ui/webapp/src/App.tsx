// SPDX-License-Identifier: Apache-2.0
import React from 'react';
import { Routes, Route, Navigate } from 'react-router-dom';
import { Button, Result, Spin } from 'antd';
import AIProfHome from './pages/AIProf';
import AIProfResultPage from './pages/AIProf/result';
import AiObservableIndex from './pages/ai_observable';
import AiObservableResult from './pages/ai_observable/result';
import AiObservableReport from './pages/ai_observable/result/Report';
import { SessionContext, useSession, loginHref } from './utils/session';

const centered: React.CSSProperties = {
  minHeight: '100vh',
  display: 'flex',
  alignItems: 'center',
  justifyContent: 'center',
};

// The report route is mounted outside this gate on purpose: report links are
// shareable, and the server does not check ownership on them either.
const SessionGate: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { user, loading, loginUrl } = useSession();

  if (loading) {
    return <div style={centered}><Spin size="large" /></div>;
  }

  if (!user) {
    const href = loginHref(loginUrl);
    return (
      <div style={centered}>
        <Result
          status="403"
          title="请先登录"
          subTitle={href ? '登录后即可查看和采集属于你的性能数据。' : '未配置登录地址，请联系管理员。'}
          extra={href ? <Button type="primary" href={href}>登录</Button> : null}
        />
      </div>
    );
  }

  return <SessionContext.Provider value={user}>{children}</SessionContext.Provider>;
};

const App: React.FC = () => (
  <Routes>
    <Route path="/ai_observable/result/Report" element={<AiObservableReport />} />
    <Route
      path="*"
      element={(
        <SessionGate>
          <Routes>
            <Route path="/" element={<Navigate to="/aiprof" replace />} />
            <Route path="/aiprof" element={<AIProfHome />} />
            <Route path="/aiprof/result" element={<AIProfResultPage />} />
            <Route path="/ai_observable" element={<AiObservableIndex />} />
            <Route path="/ai_observable/result" element={<AiObservableResult />} />
            <Route path="*" element={<Navigate to="/aiprof" replace />} />
          </Routes>
        </SessionGate>
      )}
    />
  </Routes>
);

export default App;
