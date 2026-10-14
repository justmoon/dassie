/** Switches between the system's theme, dark and light (UI-6). */

import type { JSX } from "@solidjs/web";
import { createSignal } from "solid-js";
import {
  applyTheme,
  loadTheme,
  nextTheme,
  saveTheme,
  type ThemeSetting,
} from "./theme.ts";

const LABELS: Record<ThemeSetting, string> = {
  system: "Theme: system",
  dark: "Theme: dark",
  light: "Theme: light",
};

/** The browser's storage, if the page may use it. */
function storage(): Storage | undefined {
  let found: Storage | undefined;
  try {
    found = localStorage;
  } catch {
    found = undefined;
  }
  return found;
}

const [theme, setThemeSignal] = createSignal<ThemeSetting>(
  loadTheme(storage()),
);

/** The owner's choice of theme, shared by the toggle and the settings. */
export function useTheme(): [
  () => ThemeSetting,
  (setting: ThemeSetting) => void,
] {
  return [theme, (setting) => {
    setThemeSignal(setting);
    applyTheme(setting);
    saveTheme(storage(), setting);
  }];
}

export function ThemeToggle(): JSX.Element {
  const [current, set] = useTheme();
  return (
    <button
      type="button"
      class="theme-toggle"
      data-testid="theme-toggle"
      aria-label={`${LABELS[current()]}; change`}
      onClick={() => set(nextTheme(current()))}
    >
      {current() === "dark" ? "☾" : current() === "light" ? "☀" : "◐"}
    </button>
  );
}
