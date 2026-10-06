import { defineConfig } from "vite";

export default defineConfig({
    root: "src",
    clearScreen: false,
    server: {
        host: "127.0.0.1",
        port: 24173,
        strictPort: true,
    },
    build: {
        outDir: "../dist",
        emptyOutDir: true,
        target: "es2021",
    },
});
