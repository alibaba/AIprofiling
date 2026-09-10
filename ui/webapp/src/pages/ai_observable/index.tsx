// SPDX-License-Identifier: Apache-2.0
// AI observable landing page.
// react-intl for i18n；相对路径通过 Vite 静态 /resource/ 提供。
import { PageContainer } from '@ant-design/pro-layout';
import { Tabs, Tooltip, Typography } from 'antd';
import React from 'react';
import { FormattedMessage } from 'react-intl';
import { StarAgentChannelPage } from './starAgentChannel';
import { withBase } from '../../utils/basePath';
import './index.less';

const { Paragraph } = Typography;

type TabsArrayProps = {
  label: React.ReactNode;
  key: string;
  children: React.ReactNode;
};

const ShowText: React.FC = () => (
  <div className="showText">
    <div className="showTextTitle">联系邮箱</div>
    <div className="showTextContent">
      <Paragraph style={{ marginBottom: 0 }} copyable>
        aiprof@example.com
      </Paragraph>
    </div>
  </div>
);

const AiObservableIndex: React.FC = () => {
  const items: TabsArrayProps[] = [
    {
      label: (
        <FormattedMessage id="pages.ai_performance_diagnose.staragent-channel" defaultMessage="StarAgent" />
      ),
      key: 'staragent',
      children: <StarAgentChannelPage />,
    },
  ];
  return (
    <PageContainer>
      <div className="logoTitle">
        <img src={withBase('/resource/logo/aliyun.svg')} width={36} height={36} />
        <span className="line" />
        <img src={withBase('/resource/logo/mirrorImage.png')} width={36} height={36} />
      </div>
      <Tabs type="line" items={items} destroyInactiveTabPane defaultActiveKey="staragent" />
      <Tooltip placement="right" color="#fff" title={<ShowText />}>
        <div className="email" />
      </Tooltip>
    </PageContainer>
  );
};

export default AiObservableIndex;
