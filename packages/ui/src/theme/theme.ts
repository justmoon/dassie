/**
 * Light, dark, or as the system has it (UI-6). The choice is kept in the
 * browser, and set as `data-theme` on the root, from the bundle: the CSP
 * allows no inline script, so the first paint follows the system.
 */

/** The owner's choice. */
export type ThemeSetting = "system" | "dark" | "light";

/** Where the choice is kept. */
export const THEME_KEY = "dassie.theme";

/** The stored choice, or "system". */
export function loadTheme(
  storage: Pick<Storage, "getItem"> | undefined,
): ThemeSetting {
  try {
    const stored = storage?.getItem(THEME_KEY);
    return stored === "dark" || stored === "light" ? stored : "system";
  } catch {
    return "system";
  }
}

/** Keeps the choice; storage that refuses is ignored. */
export function saveTheme(
  storage: Pick<Storage, "setItem" | "removeItem"> | undefined,
  setting: ThemeSetting,
): void {
  try {
    if (setting === "system") storage?.removeItem(THEME_KEY);
    else storage?.setItem(THEME_KEY, setting);
  } catch {
    // Private windows may refuse storage; the choice lasts the page.
  }
}

/** Applies the choice to the root element. */
export function applyTheme(
  setting: ThemeSetting,
  root: { dataset: DOMStringMap } = document.documentElement,
): void {
  if (setting === "system") delete root.dataset.theme;
  else root.dataset.theme = setting;
}

/** The next choice of the toggle. */
export function nextTheme(setting: ThemeSetting): ThemeSetting {
  return setting === "system"
    ? "dark"
    : setting === "dark"
    ? "light"
    : "system";
}
