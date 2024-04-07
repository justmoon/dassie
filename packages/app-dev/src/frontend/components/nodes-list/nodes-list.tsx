import { Plus, Wallet } from "lucide-react"
import { Link, useLocation, useRoute } from "wouter"

import { Button } from "@dassie/app-node/src/frontend/components/ui/button"
import { COLORS } from "@dassie/app-node/src/frontend/constants/palette"
import { combine } from "@dassie/app-node/src/frontend/utils/class-helper"
import { selectBySeed } from "@dassie/lib-logger"
import { useRemoteSignal } from "@dassie/lib-reactive-rpc/client"

import { rpc } from "../../utils/rpc"
import { getWalletUrl } from "../../utils/wallet-url"

interface NodeProperties {
  nodeId: string
  wallet?: boolean | undefined
  className?: string | undefined
}

const NODE_LOCATION_REGEX = /^\/nodes\/([^/]+)\/debug\/([^/]+)/

const Node = ({ nodeId, wallet, className }: NodeProperties) => {
  const [location] = useLocation()

  // If we are on a specific sub-page of another node, we want to link to that
  // sub-page on this node as well.
  const match = NODE_LOCATION_REGEX.exec(location)
  const href =
    nodeId !== "host" && match && match[1] !== nodeId
      ? `/nodes/${nodeId}/debug/${match[2]}`
      : `/nodes/${nodeId}`
  const [isActive] = useRoute(href + "/:params*")

  return (
    <div
      className={combine(
        "border rounded-md",
        isActive ? "bg-secondary" : null,
        wallet ? "grid grid-cols-[1fr_auto]" : "grid grid-cols-1",
        className,
      )}
    >
      <Button
        variant="ghost"
        className={combine(
          wallet ? "rounded-none rounded-l" : null,
          "justify-start",
        )}
        asChild
      >
        <Link href={href}>
          <i
            className="rounded-full h-2 mr-2 w-2 inline-block"
            style={{ background: selectBySeed(COLORS, nodeId) }}
          ></i>
          {nodeId}
        </Link>
      </Button>
      {wallet && (
        <Button variant="ghost" className="rounded-none rounded-r" asChild>
          <a
            href={getWalletUrl(nodeId)}
            className="flex text-gray-400 hover:text-white h-full px-2 items-center rounded-full"
            target="_blank"
            rel="noreferrer"
          >
            <Wallet className="h-4 w-4" />
          </a>
        </Button>
      )}
    </div>
  )
}

const NodesList = () => {
  const nodes = useRemoteSignal(rpc.subscribeToNodes) ?? []
  const addRandomNode = rpc.addRandomNode.useMutation()

  return (
    <div className="px-3">
      <div className="mt-4 text-lg font-semibold tracking-tight grid grid-cols-[1fr_auto]">
        <div>Nodes</div>
        <Button
          variant="ghost"
          onClick={() => {
            addRandomNode.mutate({})
          }}
        >
          <Plus className="w-4 h-4" />
          <span className="sr-only">Add node</span>
        </Button>
      </div>
      <div className="grid grid-cols-2 gap-x-2 gap-y-2">
        <Node nodeId="host" className="col-span-2" />
        {nodes.map((nodeId) => (
          <Node key={nodeId} nodeId={nodeId} wallet />
        ))}
      </div>
    </div>
  )
}

export default NodesList
