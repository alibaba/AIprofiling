// SPDX-License-Identifier: Apache-2.0
import React from 'react';
import { Layout, Tabs, Tooltip } from 'antd';
import { InfoCircleOutlined } from '@ant-design/icons';
import CapturePage from './pages/CapturePage';
import AppFooter from './components/AppFooter';
import { UserMenu } from './components/UserMenu';

const { Header, Content } = Layout;

const AIProfHome: React.FC = () => {
  return (
    <Layout style={{ minHeight: '100vh' }}>
      <Header
        style={{
          background: '#fff',
          borderBottom: '1px solid #f0f0f0',
          padding: '0 24px',
          display: 'flex',
          alignItems: 'center',
          gap: 12,
        }}
      >
        <span className="aiprof-logo">
          <span className="mark">A</span>
          AIProfiling
          <span style={{ color: '#9ca3af', fontWeight: 400, fontSize: 13, marginLeft: 6 }}>
            AI性能分析
          </span>
        </span>
        <Tooltip
          title={
            <>
              AIProf 采集本机或远端主机的 CPU/GPU 性能数据，输出火焰图、
              Kernel 分解与 AI 生成的分析建议。
            </>
          }
        >
          <InfoCircleOutlined style={{ color: '#9ca3af', marginLeft: 12 }} />
        </Tooltip>
        <UserMenu />
      </Header>
      <Content style={{ padding: 20 }}>
        <Tabs
          defaultActiveKey="agent"
          items={[
            {
              key: 'agent',
              label: '本地 Agent 通道',
              children: <CapturePage channel="agent" />,
            },
            {
              key: 'ecs',
              label: '远端主机通道',
              children: <CapturePage channel="ecs" />,
            },
          ]}
        />
      </Content>
      <AppFooter />
    </Layout>
  );
};

export default AIProfHome;
