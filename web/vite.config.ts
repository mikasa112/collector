import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

// 开发环境把 /v1 下的接口（含 /v1/ws/* websocket）转发给后端。
// 后端监听地址/端口以 collector-core/src/config/program_conf.rs 里 HttpProgram 的默认值为准，
// 实际部署配置不同时改这里即可，不影响生产环境（生产是同源嵌入，不走这个代理）。
const BACKEND = 'http://127.0.0.1:9091'

// https://vite.dev/config/
export default defineConfig({
  plugins: [vue()],
  server: {
    proxy: {
      '/v1': {
        target: BACKEND,
        ws: true,
      },
    },
  },
})
