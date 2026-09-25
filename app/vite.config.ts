import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

/** KaTeX lists every font three times (woff2, woff, ttf) for old browsers.
 *  WebKitGTK takes the first, woff2, so the other two are ~0.9 MB of dist --
 *  and of the binary Tauri embeds it in -- that nothing ever loads. Stripping
 *  them from the @font-face rules before Vite resolves the urls is what keeps
 *  those files from being emitted at all. */
function katexWoff2Only(): Plugin {
  return {
    name: "katex-woff2-only",
    enforce: "pre",
    transform(code, id) {
      if (!/katex(\.min)?\.css$/.test(id.split("?")[0])) return null;
      return code.replace(/,url\([^)]+\.(?:woff|ttf)\) format\("(?:woff|truetype)"\)/g, "");
    },
  };
}

// Tauri expects a fixed dev port; clearScreen off keeps cargo output visible.
export default defineConfig({
  plugins: [katexWoff2Only(), react(), tailwindcss()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
});
