import { defineConfig } from "vite";

export default defineConfig({
  // (relative URLs: the build can be served from any directory)
  base: "./",
  build: { target: "es2022" },
});
