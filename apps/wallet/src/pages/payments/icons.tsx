/**
 * Kopje's icons for sending and receiving, as the brand page draws them
 * (`#i-send` and `#i-receive` in apps/website/brand/index.html): strokes
 * in the color of the text beside them.
 */

import type { JSX } from "@solidjs/web";

export function SendIcon(): JSX.Element {
  return (
    <svg class="glyph" viewBox="0 0 24 24" aria-hidden="true">
      <g
        fill="none"
        stroke="currentColor"
        stroke-width="1.8"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path d="M2.5 20h19" />
        <path d="M6 15.5 16.5 5" />
        <path d="M9.5 5h7v7" />
      </g>
    </svg>
  );
}

export function ReceiveIcon(): JSX.Element {
  return (
    <svg class="glyph" viewBox="0 0 24 24" aria-hidden="true">
      <g
        fill="none"
        stroke="currentColor"
        stroke-width="1.8"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path d="M2.5 20h19" />
        <path d="M12 3.5v11.5" />
        <path d="M7.5 10.5 12 15l4.5-4.5" />
      </g>
    </svg>
  );
}
