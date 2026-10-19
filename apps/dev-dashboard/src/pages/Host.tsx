/** The dev server's own log ("host", spec/areas/dev.md#dashboard). */

import type { JSX } from "@solidjs/web";
import { createMemo } from "solid-js";
import { LogViewer } from "@dassie/ui/logs";
import { seedIndex } from "@dassie/ui/network/nodes";
import { PageTitle } from "@dassie/ui/ui";
import { useDev } from "../store.ts";
import { logSources } from "./Logs.tsx";

export function Host(): JSX.Element {
  const dev = useDev();
  const lines = createMemo(() =>
    dev.lines().filter((line) => dev.sourceOf(line) === "host")
  );
  return (
    <div class="page-wide" data-testid="page-host">
      <PageTitle>
        <span class={`dot seed-${seedIndex("host")}`} aria-hidden="true" /> Host
      </PageTitle>
      <LogViewer lines={lines} sources={logSources(dev)} />
    </div>
  );
}
