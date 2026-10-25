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

/** The switch is labelled "Dark mode" and pressed while the page is dark. */
function apply(theme: string): void {
  const dark = theme === "dark";
  root.dataset.theme = dark ? "dark" : "light";
  document.querySelector(".theme-toggle")
    ?.setAttribute("aria-pressed", String(dark));
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
