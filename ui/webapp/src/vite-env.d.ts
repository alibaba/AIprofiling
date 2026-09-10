/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_ALIYUN_CONSOLE_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
