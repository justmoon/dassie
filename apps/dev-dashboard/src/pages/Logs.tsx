/** Every log line: the dev server's and each node's. */

import type { JSX } from "@solidjs/web";
import { type LogSources, LogViewer } from "@dassie/ui/logs";
import { PageTitle } from "@dassie/ui/ui";
import { type DevStore, useDev } from "../store.ts";

/** Sources as the dashboard links them: to each node's page. */
export function logSources(dev: DevStore): LogSources {
  return {
    of: dev.sourceOf,
    href: (source) => `/nodes/${source}`,
  };
}

export function Logs(): JSX.Element {
  const dev = useDev();
  return (
    <div class="page-wide" data-testid="page-logs">
      <PageTitle>Logs</PageTitle>
      <LogViewer lines={dev.lines} sources={logSources(dev)} />
    </div>
  );
}
