import { defineConfig } from 'vite'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { plugins, build } from './vite.config.js'

const directory = dirname(fileURLToPath(import.meta.url))

export default defineConfig({
  plugins,
  build: {
    ...build,
    rollupOptions: {
      input: {
        client: resolve(directory, 'index.html'),
        // We'll never actually use this JS bundle, but need it to build assets that are only referenced by SSR pages
        ssrAssetCollector: resolve(directory, 'src/entry-server.jsx')
      }
    }
  }
})
