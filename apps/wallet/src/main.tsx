import { render } from "@solidjs/web";
import { applyTheme, loadTheme } from "@dassie/ui/theme";
import App from "./App.tsx";
import "./app.css";

/** True only in a build for the dev environment (`vite.config.ts`). */
declare const __DASSIE_DEV_LOGIN__: boolean;

// From the bundle, as the CSP allows no inline script
// (spec/areas/wallet.md#content-security-policy).
try {
  applyTheme(loadTheme(localStorage));
} catch {
  // No storage: the system's theme.
}
if (__DASSIE_DEV_LOGIN__) {
  const { devLogin } = await import("./dev-login.ts");
  await devLogin();
}
const root = document.getElementById("app");
if (!root) throw new Error("index.html has no #app element");
render(() => <App />, root);
