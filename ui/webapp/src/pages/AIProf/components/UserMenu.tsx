// SPDX-License-Identifier: Apache-2.0
import React from 'react';
import { Dropdown, Space } from 'antd';
import { UserOutlined, DownOutlined } from '@ant-design/icons';
import { useCurrentUser, loginHref } from '../../../utils/session';

// Right-aligned identity chip in the header. Hidden entirely when auth is off
// (AUTH_MODE=none): the anonymous placeholder is not a real identity, so an OSS
// deployment shows no user chip at all. It renders only once a provider-backed
// user is signed in.
export const UserMenu: React.FC = () => {
  const user = useCurrentUser();
  if (!user || user.authMode === 'none') return null;

  const name = user.nickname || user.username || user.userId;
  const items = [] as { key: string; label: React.ReactNode }[];
  if (user.logoutUrl) {
    items.push({
      key: 'logout',
      label: <a href={loginHref(user.logoutUrl) ?? user.logoutUrl}>退出登录</a>,
    });
  }

  const chip = (
    <Space style={{ cursor: items.length ? 'pointer' : 'default', color: '#4b5563' }}>
      <UserOutlined />
      {name}
      {user.isAdmin ? <span style={{ color: '#f59e0b', fontSize: 12 }}>(管理员)</span> : null}
      {items.length ? <DownOutlined style={{ fontSize: 10 }} /> : null}
    </Space>
  );

  return (
    <span style={{ marginLeft: 'auto' }}>
      {items.length ? <Dropdown menu={{ items }} trigger={['click']}>{chip}</Dropdown> : chip}
    </span>
  );
};
