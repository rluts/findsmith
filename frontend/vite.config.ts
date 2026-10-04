import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const buildTimestamp = new Date().toISOString()

export default defineConfig({
  plugins: [
    react(),
    {
      name: 'findsmith-build-timestamp',
      transformIndexHtml() {
        return [{
          tag: 'meta',
          attrs: { name: 'findsmith-build', content: buildTimestamp },
          injectTo: 'head',
        }]
      },
    },
  ],
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:8000',
      '/oauth': 'http://127.0.0.1:8000',
    },
  },
})
