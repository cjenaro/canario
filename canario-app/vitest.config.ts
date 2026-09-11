import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

// Tests run in node, where solid-js's "node"/default export conditions resolve
// to its SERVER build — in which onMount/createEffect never fire. Pin the
// import to the client build so component-lifecycle primitives behave like
// they do in the real renderer.
const solidClient = fileURLToPath(new URL("./node_modules/solid-js/dist/solid.js", import.meta.url));

export default defineConfig({
  resolve: {
    alias: [{ find: /^solid-js$/, replacement: solidClient }],
  },
  ssr: {
    // Keep solid-js out of SSR externalization so the alias above applies.
    noExternal: ["solid-js"],
  },
  test: {
    include: ["src/**/*.test.ts", "scripts/**/*.test.mjs"],
  },
});
