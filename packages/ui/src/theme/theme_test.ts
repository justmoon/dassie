import { assertEquals } from "@std/assert";
import {
  applyTheme,
  loadTheme,
  nextTheme,
  saveTheme,
  THEME_KEY,
} from "./theme.ts";

Deno.test("the choice is kept, and anything else reads as system", () => {
  const store = new Map<string, string>();
  const storage = {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => void store.set(key, value),
    removeItem: (key: string) => void store.delete(key),
  };
  assertEquals(loadTheme(storage), "system");
  saveTheme(storage, "dark");
  assertEquals(loadTheme(storage), "dark");
  store.set(THEME_KEY, "purple");
  assertEquals(loadTheme(storage), "system");
  saveTheme(storage, "system");
  assertEquals(store.has(THEME_KEY), false);
  const refusing = {
    getItem: () => {
      throw new Error("denied");
    },
  };
  assertEquals(loadTheme(refusing), "system");
});

Deno.test("the toggle goes round, and the root follows", () => {
  assertEquals(nextTheme("system"), "dark");
  assertEquals(nextTheme("dark"), "light");
  assertEquals(nextTheme("light"), "system");
  const root = { dataset: {} as DOMStringMap };
  applyTheme("dark", root);
  assertEquals(root.dataset.theme, "dark");
  applyTheme("system", root);
  assertEquals(root.dataset.theme, undefined);
});
