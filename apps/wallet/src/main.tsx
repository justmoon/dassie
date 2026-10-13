import { render } from "@solidjs/web";
import App from "./App.tsx";
import "./app.css";

const root = document.getElementById("app");
if (!root) throw new Error("index.html has no #app element");
render(() => <App />, root);
