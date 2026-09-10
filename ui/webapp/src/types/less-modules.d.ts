// SPDX-License-Identifier: Apache-2.0
// 让 TS 认识 `import styles from './xxx.less'` 这种 CSS Modules 写法。
declare module '*.less' {
  const classes: { readonly [key: string]: string };
  export default classes;
}
declare module '*.css' {
  const classes: { readonly [key: string]: string };
  export default classes;
}
