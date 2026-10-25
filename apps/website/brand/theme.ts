/**
 * The brand page's switch between Kopje's light theme and its dark one,
 * dusk. The page starts light; the reader's choice is kept, and
 * `?theme=dark` or `?theme=light` picks one for a link.
 *
 * @module
 */

const KEY = "dassie-brand-theme";
const root = document.documentElement;

function chosen(): string | null {
  const asked = new URLSearchParams(location.search).get("theme");
  if (asked === "dark" || asked === "light") return asked;
  try {
    return localStorage.getItem(KEY);
  } catch {
    // Storage may be off; the page then starts light every time.
    return null;
  }
}

function apply(theme: string): void {
  const dark = theme === "dark";
  root.dataset.theme = dark ? "dark" : "light";
  const button = document.querySelector<HTMLButtonElement>(".theme-toggle");
  if (!button) return;
  button.setAttribute("aria-pressed", String(dark));
  const label = button.querySelector("span");
  if (label) label.textContent = dark ? "Light mode" : "Dark mode";
}

apply(chosen() ?? "light");

document.querySelector(".theme-toggle")?.addEventListener("click", () => {
  const next = root.dataset.theme === "dark" ? "light" : "dark";
  apply(next);
  try {
    localStorage.setItem(KEY, next);
  } catch {
    // Not kept, but the page still switches.
  }
});
