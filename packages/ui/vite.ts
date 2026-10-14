/**
 * How the wallet and the dev dashboard resolve the workspace in Vite.
 *
 * Vite resolves imports itself, not through Deno, so it is told where Deno
 * found each workspace module a page imports. Asking Deno
 * (`import.meta.resolve`) keeps the pages out of the packages' directories:
 * they only know names and exports.
 *
 * The configs that use this are loaded natively (`--configLoader native`),
 * so Deno runs them in place and resolves through the workspace. Vite's
 * default loader bundles a config into a temporary module first, where
 * `import.meta.resolve` no longer yields a file URL.
 *
 * @module
 */

import { fileURLToPath } from "node:url";
import type { Alias, ResolveOptions } from "vite";
import manifest from "./deno.json" with { type: "json" };

/** The other packages' modules the pages import. */
const OTHERS = [
  "@dassie/rpc",
  "@dassie/rpc/proto",
  "@dassie/rpc/dev",
  "@dassie/identity-wasm/web",
];

function exact(specifier: string, path: string): Alias {
  const escaped = specifier.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
  return { find: new RegExp(`^${escaped}$`), replacement: path };
}

/**
 * Aliases for every export of `@dassie/ui` and the other packages, and one
 * copy of Solid: a second would have contexts of its own, which the other
 * copy's components never see.
 */
export function workspaceResolve(): ResolveOptions & { alias: Alias[] } {
  const own = Object.entries(manifest.exports)
    .filter(([key]) => !key.startsWith("./testing/") && key !== "./vite")
    .map(([key, path]) =>
      exact(
        `@dassie/ui${key.slice(1)}`,
        fileURLToPath(new URL(path, import.meta.url)),
      )
    );
  const others = OTHERS.map((specifier) =>
    exact(specifier, fileURLToPath(import.meta.resolve(specifier)))
  );
  return {
    alias: [...own, ...others],
    dedupe: ["solid-js", "@solidjs/web"],
  };
}
