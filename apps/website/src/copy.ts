/**
 * The home page's Copy button, which copies the install command. Without
 * script, or without a clipboard the page may write to, it stays hidden,
 * and the command is there to select.
 *
 * @module
 */

for (
  const button of document.querySelectorAll<HTMLButtonElement>("[data-copy]")
) {
  const text = button.dataset.copy;
  const label = button.querySelector("span");
  if (!text || !label || !navigator.clipboard) continue;
  button.hidden = false;
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text);
      label.textContent = "Copied";
    } catch {
      label.textContent = "Select it to copy";
    }
    setTimeout(() => {
      label.textContent = "Copy";
    }, 2000);
  });
}
