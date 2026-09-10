// SPDX-License-Identifier: Apache-2.0
import React from 'react';
import { Layout } from 'antd';

const { Footer } = Layout;

const AppFooter: React.FC = () => (
  <Footer
    style={{
      background: 'transparent',
      textAlign: 'center',
      color: '#9ca3af',
      fontSize: 12,
      padding: '16px 24px',
    }}
  >
    Powered by Alibaba Cloud
  </Footer>
);

export default AppFooter;
