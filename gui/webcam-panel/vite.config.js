import { defineConfig } from 'vite'

// `base: './'` matters: the built panel is deployed into the q-flux dist root
// alongside dozens of other surfaces, and may be served from a subpath. Absolute
// asset URLs would 404 there.
export default defineConfig({
  base: './',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // One file each — the deploy path (flux_ui_deploy / additive copy) is far
    // simpler when the panel is not a fan of hashed chunks.
    rollupOptions: {
      output: {
        entryFileNames: 'assets/panel.js',
        chunkFileNames: 'assets/panel-[hash].js',
        assetFileNames: 'assets/panel.[ext]',
      },
    },
  },
  server: { host: '127.0.0.1', port: 5178 },
})
