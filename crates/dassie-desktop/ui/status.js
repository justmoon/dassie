// The app shows this page while its node starts, and with `#error=<text>`
// when the node could not start (crates/dassie-desktop/src/window.rs).
const error = new URLSearchParams(location.hash.slice(1)).get("error")
if (error !== null) {
  document.getElementById("title").textContent = "Your node could not start"
  document.getElementById("detail").textContent = error
}
