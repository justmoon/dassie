import { render } from "@solidjs/web";
import { applyTheme, loadTheme } from "@dassie/ui/theme";
import App from "./App.tsx";
import "./app.css";

// From the bundle, as the CSP allows no inline script.
try {
  applyTheme(loadTheme(localStorage));
} catch {
  // No storage: the system's theme.
}
const root = document.getElementById("app");
if (!root) throw new Error("index.html has no #app element");
render(() => <App />, root);
