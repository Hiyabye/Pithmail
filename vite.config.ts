import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  build: { target: ['safari15', 'chrome105', 'firefox102'] },
  server: { host: '127.0.0.1', port: 1420, strictPort: true },
});
