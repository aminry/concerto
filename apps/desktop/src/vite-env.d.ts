/// <reference types="vite/client" />

// Monaco's ESM worker entry points are imported with Vite's `?worker`
// query suffix. Vite's stock `vite/client` declarations cover plain
// `?worker` imports, but the Monaco paths are deep file references
// without a TS shim — declare them here so `tsc --noEmit` is happy.
declare module "monaco-editor/esm/vs/editor/editor.worker?worker" {
  const Worker: new () => Worker;
  export default Worker;
}
