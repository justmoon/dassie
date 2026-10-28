/**
 * Dassie's lockup: the mark beside the wordmark, as the site's header has
 * it (apps/website/brand/index.html). The mark is the file the brand page
 * keeps, never redrawn; its copy here is checked against it
 * (`kopje_test.ts`).
 */

import type { JSX } from "@solidjs/web";

const MARK = new URL("../../kopje/dassie-mark.svg", import.meta.url).href;

export function Lockup(props: { suffix?: string }): JSX.Element {
  return (
    <span class="lockup">
      <img src={MARK} alt="" width="34" height="33" />
      <span class="wordmark">Dassie</span>
      {props.suffix && <span class="lockup-suffix">{props.suffix}</span>}
    </span>
  );
}
