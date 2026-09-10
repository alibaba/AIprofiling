// SPDX-License-Identifier: Apache-2.0
// Session probe against GET /api/me. In AUTH_MODE=none the server answers with
// the anonymous user, so the login affordance never renders for OSS users.
import { useEffect, useState, createContext, useContext } from 'react';
import { GetSession, type SessionUser } from '../api';

export type { SessionUser };

// Populated by the gate in App.tsx, so pages below it can read the signed-in
// user without each running its own /api/me probe.
export const SessionContext = createContext<SessionUser | null>(null);

export function useCurrentUser() {
  return useContext(SessionContext);
}

export function useSession() {
  const [user, setUser] = useState<SessionUser | null>(null);
  const [loginUrl, setLoginUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    GetSession()
      .then((resp) => {
        if (cancelled) return;
        if (resp.code === 'Success' && resp.data) {
          setUser(resp.data);
          setLoginUrl(resp.data.loginUrl ?? null);
        } else {
          setUser(null);
          setLoginUrl(resp.loginUrl ?? null);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => { cancelled = true; };
  }, []);

  return { user, loading, loginUrl };
}

export function loginHref(loginUrl: string | null): string | null {
  if (!loginUrl) return null;
  const sep = loginUrl.includes('?') ? '&' : '?';
  return `${loginUrl}${sep}redirect=${encodeURIComponent(window.location.href)}`;
}
