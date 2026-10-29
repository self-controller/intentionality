import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri expects a fixed dev port; clearScreen off keeps cargo output visible.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  // `@/` is src/, the shadcn convention, so components pasted from shadcn and
  // ReUI keep their imports (`@/components/ui/...`, `@/lib/utils`) as written.
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
});
