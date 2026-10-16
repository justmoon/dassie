import { render } from "@solidjs/web";
import { MAP } from "./map.ts";
import { Architecture } from "./Architecture.tsx";

const root = document.getElementById("architecture");
if (!root) throw new Error("the page has no #architecture element");
root.replaceChildren();
render(() => <Architecture map={MAP} />, root);
