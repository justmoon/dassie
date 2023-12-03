import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { createWSClient, wsLink } from "@trpc/client"
import { Link } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import superjson from "superjson"
import { Route, Router, useLocation, useRouter } from "wouter"

import { Button } from "@dassie/app-node/src/frontend/components/ui/button"
import {
  Tabs,
  TabsList,
  TabsTrigger,
} from "@dassie/app-node/src/frontend/components/ui/tabs"
import { DebugPage } from "@dassie/app-node/src/frontend/pages/debug/debug"
import {
  queryClientReactContext,
  trpc as trpcNode,
} from "@dassie/app-node/src/frontend/utils/trpc"
import { selectBySeed } from "@dassie/lib-logger"

import { COLORS } from "../../../constants/palette"
import { trpc as trpcDevelopment } from "../../../utils/trpc"
import { getWalletUrl } from "../../../utils/wallet-url"
import LogViewer from "../../log-viewer/log-viewer"

interface BasicNodeElementProperties {
  nodeId: string
}

const NodeHeader = ({ nodeId }: BasicNodeElementProperties) => {
  const color = useMemo(() => selectBySeed(COLORS, nodeId), [nodeId])
  return (
    <header>
      <h1 className="font-bold leading-tight text-3xl px-4">
        <i
          className="rounded-full h-5 mr-4 w-5 inline-block"
          style={{ background: color }}
        ></i>
        Node:{" "}
        <span
          style={{
            color,
          }}
        >
          {nodeId}
        </span>
        <Button asChild variant="outline" className="ml-4">
          <a href={getWalletUrl(nodeId)} target="_blank" rel="noreferrer">
            <Link className="h-4 w-4 mr-2" />
            Wallet
          </a>
        </Button>
      </h1>
    </header>
  )
}

const NodeLogViewer = ({ nodeId }: BasicNodeElementProperties) => {
  return <LogViewer filter={({ node }) => node === nodeId} />
}

const createNodeTrpcClients = (securityToken: string, nodeId: string) => {
  const queryClient = new QueryClient()
  const wsClient = createWSClient({
    url: `wss://${nodeId}.localhost/trpc?token=${securityToken}`,
  })
  const trpcClient = trpcNode.createClient({
    links: [wsLink({ client: wsClient })],
    transformer: superjson,
  })

  return { queryClient, wsClient, trpcClient }
}

interface NodeDetailProperties extends BasicNodeElementProperties {
  secondaryParameters: string | undefined
}

const NodeDetail = ({ nodeId, secondaryParameters }: NodeDetailProperties) => {
  const router = useRouter()
  const [, setLocation] = useLocation()

  const currentTab = secondaryParameters?.split("/")[1] ?? "logs"

  function onTabChange(value: string) {
    setLocation(`/nodes/${nodeId}/debug/${value}`)
  }

  const [clients, setClients] = useState<
    ReturnType<typeof createNodeTrpcClients> | undefined
  >(undefined)

  const { data: securityToken } = trpcDevelopment.ui.getSecurityToken.useQuery()

  useEffect(() => {
    if (!securityToken) return

    const clients = createNodeTrpcClients(securityToken, nodeId)
    setClients(clients)

    const handleConnect = () => {
      clients.wsClient
        .getConnection()
        .removeEventListener("open", handleConnect)
    }
    clients.wsClient.getConnection().addEventListener("open", handleConnect)

    return () => {
      clients.wsClient
        .getConnection()
        .removeEventListener("open", handleConnect)
      clients.wsClient.close()
    }
  }, [securityToken, nodeId])

  if (!clients) return null

  return (
    <Router base={`/nodes/${nodeId}`} parent={router}>
      <trpcNode.Provider
        client={clients.trpcClient}
        queryClient={clients.queryClient}
      >
        <QueryClientProvider
          client={clients.queryClient}
          context={queryClientReactContext}
        >
          <div className="h-screen grid grid-rows-[auto_1fr] gap-4 py-10">
            <NodeHeader nodeId={nodeId} />
            <Tabs
              value={currentTab}
              onValueChange={onTabChange}
              className="min-h-0 grid grid-rows-[auto_1fr] px-4"
            >
              <TabsList className="justify-start mb-2">
                <TabsTrigger value="logs">Logs</TabsTrigger>
                <TabsTrigger value="nodes">Nodes</TabsTrigger>
                <TabsTrigger value="ledger">Ledger</TabsTrigger>
                <TabsTrigger value="routing">Routing</TabsTrigger>
                <TabsTrigger value="state">State</TabsTrigger>
                <TabsTrigger value="database">Database</TabsTrigger>
              </TabsList>
              <Route path="/debug/logs">
                {() => <NodeLogViewer nodeId={nodeId} />}
              </Route>
              <Route path="/debug/:params*" component={DebugPage} />
              <Route>{() => <NodeLogViewer nodeId={nodeId} />}</Route>
            </Tabs>
          </div>
        </QueryClientProvider>
      </trpcNode.Provider>
    </Router>
  )
}

export default NodeDetail
