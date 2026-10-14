/** QR codes as modules, for drawing (spec 7 §5). */

import { encode } from "uqr";

/** The dark modules of `text`'s QR code, row by row, with its size. */
export function qrModules(
  text: string,
): { size: number; dark: [number, number][] } {
  const code = encode(text, { ecc: "M", border: 2 });
  const dark: [number, number][] = [];
  code.data.forEach((row, y) =>
    row.forEach((on, x) => {
      if (on) dark.push([x, y]);
    })
  );
  return { size: code.size, dark };
}
