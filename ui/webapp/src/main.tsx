// SPDX-License-Identifier: Apache-2.0
import React from 'react';
import ReactDOM from 'react-dom/client';
import { ConfigProvider } from 'antd';
import antdZhCN from 'antd/locale/zh_CN';
import antdEnUS from 'antd/locale/en_US';
import { BrowserRouter } from 'react-router-dom';
import { IntlProvider } from 'react-intl';
import App from './App';
import zhCN from './locales/zh-CN';
import enUS from './locales/en-US';
import 'antd/dist/reset.css';
import './styles/global.css';

const lang = (navigator.language || 'zh-CN').toLowerCase();
const isZh = lang.startsWith('zh');
const messages = isZh ? zhCN : enUS;
const locale = isZh ? 'zh-CN' : 'en-US';
const antdLocale = isZh ? antdZhCN : antdEnUS;

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <IntlProvider
      locale={locale}
      defaultLocale="zh-CN"
      messages={messages}
      onError={() => { /* swallow missing-key warnings during migration */ }}
    >
      <ConfigProvider
        locale={antdLocale}
        theme={{ token: { colorPrimary: '#1677ff', borderRadius: 8 } }}
      >
        <BrowserRouter basename={import.meta.env.BASE_URL}>
          <App />
        </BrowserRouter>
      </ConfigProvider>
    </IntlProvider>
  </React.StrictMode>,
);
