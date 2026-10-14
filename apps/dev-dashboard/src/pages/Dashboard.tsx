/** The dashboard's front page: the network, and every node's log. */

import type { JSX } from "@solidjs/web";
import { create } from "@bufbuild/protobuf";
import { NetworkNodeSchema } from "@dassie/rpc/proto";
import { LogViewer } from "@dassie/ui/logs";
import { NetworkGraph } from "@dassie/ui/network";
import { useRouter } from "@dassie/ui/router";
import { Card, PageTitle } from "@dassie/ui/ui";
import { createMemo } from "solid-js";
import { linkKey, mutualLinks } from "../graph.ts";
import { useDev } from "../store.ts";
import { logSources } from "./Logs.tsx";

export function Dashboard(): JSX.Element {
  const dev = useDev();
  const router = useRouter();
  const nodes = createMemo(() =>
    dev.state().nodes.map((node) =>
      create(NetworkNodeSchema, { id: node.name, alias: node.name })
    )
  );
  const links = createMemo(() => mutualLinks(dev.peers()));
  return (
    <div class="page-wide" data-testid="page-dashboard">
      <PageTitle>Dashboard</PageTitle>
      <Card>
        <NetworkGraph
          nodes={nodes()}
          links={links()}
          selfId=""
          hrefFor={(id) => router.href(`/nodes/${id}`)}
          onSelect={(id) => router.navigate(`/nodes/${id}`)}
          active={(link) => dev.lit().has(linkKey(link.a, link.b))}
        />
      </Card>
      <LogViewer lines={dev.lines} sources={logSources(dev)} />
    </div>
  );
}
