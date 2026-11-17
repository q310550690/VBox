import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { fileURLToPath, URL } from "node:url";
import { readFileSync } from "node:fs";

// 构建期注入应用版本（关于页 / 浏览器预览模式显示用；桌面端仍以 Tauri 的版本为准）
const pkg = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
) as { version: string };

// Vite 配置：Vue 3 + Tauri 2 桌面端
// 该文件由 `cargo create-tauri-app` 生成的官方脚手架调整而来：
// 保留了端口 1420 / strictPort / clearScreen:false 等 Tauri 必需项，
// 额外补充了 base:'./'（打包后相对路径）与 '@' 别名（对应 tsconfig 的 paths）。
export default defineConfig(() => {
  // 允许通过 TAURI_DEV_HOST 指定移动端真机调试的宿主地址
  const host = process.env.TAURI_DEV_HOST;

  return {
    plugins: [vue()],
    // 构建期常量：应用版本（Tauri 环境读不到时作为兜底 / 浏览器预览显示）
    define: {
      __APP_VERSION__: JSON.stringify(pkg.version),
    },
    // Tauri 打包后资源用相对路径，便于定位
    base: "./",
    resolve: {
      alias: {
        "@": fileURLToPath(new URL("./src", import.meta.url)),
      },
    },
    // 防止 Vite 清屏遮住 Rust 报错
    clearScreen: false,
    // Tauri 期望固定端口，且严格占用，防止漂移导致拉不起前端
    server: {
      port: 1420,
      strictPort: true,
      host: host || false,
      hmr: host
        ? { protocol: "ws", host, port: 1421 }
        : undefined,
      watch: {
        // 忽略 Rust 端目录，避免无谓重建
        ignored: ["**/src-tauri/**"],
      },
    },
    envPrefix: ["VITE_", "TAURI_"],
  };
});
