// axios wrapper
import axios, { AxiosHeaders } from 'axios';

// baseURL matches the deployment's BASE_PATH so `request.get('/api/x')` becomes
// e.g. `/aiprof-open/api/x` and hits the correct reverse-proxy location.
// Vite's `BASE_URL` is trailing-slashed; strip it so callers writing "/api"
// don't produce "//api".
const baseURL = (import.meta.env.BASE_URL || '/').replace(/\/+$/, '');

const instance = axios.create({
  baseURL,
  timeout: 30000,
});

instance.interceptors.request.use(
  (config) => {
    const headers = AxiosHeaders.from(config.headers as never);
    const token = localStorage.getItem('token');
    if (token) headers.set('Authorization', token);
    if (!headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
    config.headers = headers;
    return config;
  },
  (error) => Promise.reject(error),
);

instance.interceptors.response.use(
  (response) => response.data,
  (error) => {
    console.error('Request Error:', error);
    return Promise.reject(error);
  },
);

export default instance;
