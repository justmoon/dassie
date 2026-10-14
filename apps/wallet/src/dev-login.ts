/**
 * The dev environment's auto-login links (spec 7 §6): the dashboard links
 * to `https://dN.localhost/#dassie-dev-login=<proof>`, with the node's
 * login proof in the fragment, which never reaches a server. A wallet built
 * with `--mode development` sends it as an ordinary login and drops it from
 * the URL; the node checks it as any other. A production build has none of
 * this: `main.tsx` imports it behind `__DASSIE_DEV_LOGIN__`.
 */

import { submitLoginProof } from "./auth.ts";

const PREFIX = "#dassie-dev-login=";

/** The proof in a URL fragment, if it holds one: 32 bytes as hex. */
export function devLoginProof(hash: string): string | undefined {
  if (!hash.startsWith(PREFIX)) return undefined;
  const proof = hash.slice(PREFIX.length).toLowerCase();
  return /^[0-9a-f]{64}$/.test(proof) ? proof : undefined;
}

/** Logs in with the proof in this page's URL, if there is one. */
export async function devLogin(): Promise<void> {
  const proof = devLoginProof(location.hash);
  if (proof === undefined) return;
  // The proof is a credential: out of the URL and the history first.
  history.replaceState(null, "", location.pathname + location.search);
  const outcome = await submitLoginProof(proof);
  if (!outcome.ok) {
    console.warn(`dev login failed (${outcome.status}): ${outcome.message}`);
  }
}
