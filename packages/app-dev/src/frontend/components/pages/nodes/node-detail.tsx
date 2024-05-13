import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { Link } from "lucide-react"
import { useMemo } from "react"
import superjson from "superjson"
import { Redirect, Route, useLocation } from "wouter"

import type { AppRouter } from "@dassie/app-node/src/backend/rpc-server/app-router"
import { Button } from "@dassie/app-node/src/frontend/components/ui/button"
import {
  Tabs,
  TabsList,
  TabsTrigger,
} from "@dassie/app-node/src/frontend/components/ui/tabs"
import { COLORS } from "@dassie/app-node/src/frontend/constants/palette"
import { DebugPage } from "@dassie/app-node/src/frontend/pages/debug/debug"
import { RpcProvider as NodeRpcProvider } from "@dassie/app-node/src/frontend/utils/rpc"
import { selectBySeed } from "@dassie/lib-logger"
import { createClient, createWebSocketLink } from "@dassie/lib-rpc/client"

import { rpc as rpcDevelopment } from "../../../utils/rpc"
import { getWalletUrl } from "../../../utils/wallet-url"
import {
  DevelopmentLogProvider,
  DevelopmentLogViewer,
} from "../../development-log-viewer/development-log-viewer"

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

const createNodeRpcClients = (securityToken: string, nodeId: string) => {
  const queryClient = new QueryClient()
  const websocket = new WebSocket(
    `wss://${nodeId}.localhost/trpc?token=${securityToken}`,
  )
  const rpcClient = createClient<AppRouter>({
    connection: createWebSocketLink(websocket),
    transformer: superjson,
  })

  return { queryClient, rpcClient }
}

interface NodeDetailProperties extends BasicNodeElementProperties {}

const NodeDetail = ({ nodeId }: NodeDetailProperties) => {
  const [location, setLocation] = useLocation()
  const currentTab = /^\/debug\/(.*)$/.exec(location)?.[1] ?? "logs"

  function onTabChange(value: string) {
    setLocation(`/debug/${value}`)
  }

  const { data: securityToken } = rpcDevelopment.getSecurityToken.useQuery()

  const clients = useMemo(
    () => securityToken && createNodeRpcClients(securityToken, nodeId),
    [securityToken, nodeId],
  )

  if (!clients) return null

  return (
    <NodeRpcProvider
      rpcClient={clients.rpcClient}
      queryClient={clients.queryClient}
    >
      <QueryClientProvider client={clients.queryClient}>
        <DevelopmentLogProvider>
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
              {currentTab === "logs" ? (
                <DevelopmentLogViewer filter={({ node }) => node === nodeId} />
              ) : (
                <Route path={`/debug`} nest>
                  <DebugPage key={nodeId} />
                </Route>
              )}
              <Route path={`/`}>
                {() => <Redirect to={`/debug/logs`} replace />}
              </Route>
            </Tabs>
          </div>
        </DevelopmentLogProvider>
      </QueryClientProvider>
    </NodeRpcProvider>
  )
}

export default NodeDetail
