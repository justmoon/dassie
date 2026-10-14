import { render } from "@solidjs/web";
import App from "./App.tsx";
import { applyTheme, loadTheme } from "./theme.ts";
import "./app.css";

// From the bundle, as the CSP allows no inline script (spec 7 §5).
try {
  applyTheme(loadTheme(localStorage));
} catch {
  // No storage: the system's theme.
}
const root = document.getElementById("app");
if (!root) throw new Error("index.html has no #app element");
render(() => <App />, root);
