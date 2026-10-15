import { render } from "@solidjs/web";
import { loadStream } from "@dassie/identity-wasm/web/stream";
import { Toy } from "./Toy.tsx";

const root = document.getElementById("toy");
if (!root) throw new Error("the page has no #toy element");
try {
  const stream = await loadStream();
  root.replaceChildren();
  render(() => <Toy stream={stream} />, root);
} catch (error) {
  const message = document.createElement("p");
  message.className = "error";
  message.dataset.testid = "toy-error";
  message.textContent = `The STREAM toy could not load: ${error}`;
  root.replaceChildren(message);
}
