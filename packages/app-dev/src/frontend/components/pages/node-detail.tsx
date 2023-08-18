import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { createWSClient, wsLink } from "@trpc/client"
import { Link } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import superjson from "superjson"

import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@dassie/app-node/src/frontend/components/ui/tabs"
import { Ledger } from "@dassie/app-node/src/frontend/pages/debug/ledger/ledger"
import { Nodes } from "@dassie/app-node/src/frontend/pages/debug/nodes/nodes"
import { Routing } from "@dassie/app-node/src/frontend/pages/debug/routing/routing"
import {
  queryClientReactContext,
  trpc,
} from "@dassie/app-node/src/frontend/utils/trpc"
import { selectBySeed } from "@dassie/lib-logger"

import { COLORS } from "../../constants/palette"
import { getWalletUrl } from "../../utils/wallet-url"
import LogViewer from "../log-viewer/log-viewer"

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
        <a
          href={getWalletUrl(nodeId)}
          target="_blank"
          className="ml-8 text-gray text-lg"
          rel="noreferrer"
        >
          Wallet
          <Link className="inline-block ml-2" />
        </a>
      </h1>
    </header>
  )
}

const NodeLogViewer = ({ nodeId }: BasicNodeElementProperties) => {
  return <LogViewer filter={({ node }) => node === nodeId} />
}

const createNodeTrpcClients = (nodeId: string) => {
  const queryClient = new QueryClient()
  const wsClient = createWSClient({
    url: `wss://${nodeId}.localhost/trpc`,
  })
  const trpcClient = trpc.createClient({
    links: [wsLink({ client: wsClient })],
    transformer: superjson,
  })

  return { queryClient, wsClient, trpcClient }
}

const NodeDetail = ({ nodeId }: BasicNodeElementProperties) => {
  const [activeTab, onTabChange] = useState("logs")

  const [clients, setClients] = useState<
    ReturnType<typeof createNodeTrpcClients> | undefined
  >(undefined)

  const [ready, setReady] = useState(false)

  useEffect(() => {
    const clients = createNodeTrpcClients(nodeId)
    setClients(clients)

    const handleConnect = () => {
      setReady(true)
      clients.wsClient
        .getConnection()
        .removeEventListener("open", handleConnect)
    }
    clients.wsClient.getConnection().addEventListener("open", handleConnect)

    return () => {
      setReady(false)
      clients.wsClient
        .getConnection()
        .removeEventListener("open", handleConnect)
      clients.wsClient.close()
    }
  }, [nodeId])

  if (!clients) return null

  return (
    <trpc.Provider
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
            value={activeTab}
            onValueChange={onTabChange}
            className="min-h-0 grid grid-rows-[auto_1fr] px-4"
          >
            <TabsList className="justify-start">
              <TabsTrigger value="logs">Logs</TabsTrigger>
              <TabsTrigger value="nodes">Nodes</TabsTrigger>
              <TabsTrigger value="ledger">Ledger</TabsTrigger>
              <TabsTrigger value="routing">Routing</TabsTrigger>
            </TabsList>
            <TabsContent value="logs" className="min-h-0">
              <NodeLogViewer nodeId={nodeId} />
            </TabsContent>
            <TabsContent value="nodes">{ready && <Nodes />}</TabsContent>
            <TabsContent value="ledger">{ready && <Ledger />}</TabsContent>
            <TabsContent value="routing">{ready && <Routing />}</TabsContent>
          </Tabs>
        </div>
      </QueryClientProvider>
    </trpc.Provider>
  )
}

export default NodeDetail
