import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import("@sveltejs/vite-plugin-svelte").SvelteConfig} */
export default {
  preprocess: vitePreprocess(),
  vitePlugin: {
    // Runes mode for our own components; dependencies keep whatever they use.
    dynamicCompileOptions({ filename }) {
      if (!filename.includes("node_modules")) return { runes: true };
    },
  },
};
