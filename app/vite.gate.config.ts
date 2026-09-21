import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { viteSingleFile } from "vite-plugin-singlefile";

/** The gate's bundle.
 *
 *  One self-contained index.html, because gate/gui.py hands it to
 *  WebKit.load_html() and because a login-critical artifact that lives in git
 *  should be one reviewable file, not a directory of content-hashed assets
 *  whose names churn on every build.
 *
 *  Output goes to gate/webui/, deliberately not named dist/ -- .gitignore has
 *  a bare `dist/` line that matches at any depth, and this one must be
 *  committed: login cannot depend on node_modules being present. */
export default defineConfig({
  root: "gate-ui",
  plugins: [react(), tailwindcss(), viteSingleFile()],
  build: {
    outDir: "../../gate/webui",
    emptyOutDir: true,
    assetsInlineLimit: 100_000_000,
    cssCodeSplit: false,
  },
});
