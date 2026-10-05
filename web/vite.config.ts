import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// There is no dev-server proxy here on purpose: the panel is built and run on
// the homelab (scripts/dev.sh deploy), never served from the workstation.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    target: 'es2022',
    // Vite would otherwise inline small files as data: URLs. The page's content
    // security policy allows this origin only, so an inlined font is a blocked font.
    assetsInlineLimit: 0,
  },
})
