/**
 * A QR code, drawn as SVG in the page: no image from elsewhere, so the
 * strict CSP holds. For deposit addresses (spec/areas/wallet.md#ledgers).
 */

import type { JSX } from "@solidjs/web";
import { createMemo, For } from "solid-js";
import { qrModules } from "./qr.ts";

/** `text` as a QR code, `pixels` wide. */
export function Qr(
  props: { text: string; label: string; pixels?: number; testid?: string },
): JSX.Element {
  const code = createMemo(() => qrModules(props.text));
  return (
    <svg
      class="qr"
      role="img"
      aria-label={props.label}
      data-testid={props.testid}
      width={props.pixels ?? 160}
      height={props.pixels ?? 160}
      viewBox={`0 0 ${code().size} ${code().size}`}
      shape-rendering="crispEdges"
    >
      <rect width={code().size} height={code().size} fill="#fff" />
      <For each={code().dark}>
        {([x, y]) => <rect x={x} y={y} width="1" height="1" fill="#000" />}
      </For>
    </svg>
  );
}
