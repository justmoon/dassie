import { assertEquals } from "@std/assert";
import { qrModules } from "./qr.ts";

Deno.test("an XRPL address fits a small QR code, with its finder patterns", () => {
  const code = qrModules("rH4kpiQ6LktHwBz724utTcDQqDw5YRcX48");
  // Version 3 (29 modules) with a border of 2 each side.
  assertEquals(code.size, 33);
  const dark = new Set(code.dark.map(([x, y]) => `${x},${y}`));
  // The top-left finder's outer ring starts inside the border.
  assertEquals(dark.has("2,2"), true);
  assertEquals(dark.has("8,2"), true);
  assertEquals(dark.has("1,1"), false);
  // The same text gives the same code.
  assertEquals(qrModules("rH4kpiQ6LktHwBz724utTcDQqDw5YRcX48"), code);
});
