import { FaPlus, FaWallet } from "react-icons/fa"
import { Link } from "wouter"

import { selectBySeed } from "@dassie/lib-logger"
import { useRemoteSignal } from "@dassie/lib-reactive-trpc/client"

import { COLORS } from "../../constants/palette"
import { trpc } from "../../utils/trpc"
import PeeringModeToggle from "../pages/dashboard/peering-mode-toggle"
import { HOST_COLOR } from "../pages/host-detail"

const NodesList = () => {
  const nodes = [...(useRemoteSignal(trpc.ui.subscribeToNodes) ?? new Set())]
  const addRandomNode = trpc.ui.addRandomNode.useMutation()

  return (
    <div className="rounded-lg flex flex-col bg-gray-800 min-h-0 pt-4">
      <PeeringModeToggle />
      <div className="flex justify-between items-end">
        <h2 className="font-bold text-xl">Nodes</h2>
        <button
          type="button"
          className="rounded font-medium bg-blue-700 text-white text-xs text-center mr-2 p-1 inline-flex items-center dark:bg-blue-600 hover:bg-blue-800 focus:outline-none focus:ring-4 focus:ring-blue-300 dark:hover:bg-blue-700 dark:focus:ring-blue-800"
          onClick={() => {
            addRandomNode.mutate({})
          }}
        >
          <FaPlus className="flex-shrink-0 text-lg" />
          <span className="sr-only">Add node</span>
        </button>
      </div>
      <div className="grid grid-cols-2 py-4 gap-x-2 gap-y-3">
        <div className="flex gap-2 items-center bg-slate-700 rounded-full col-span-2">
          <Link
            href="/nodes/host"
            className="flex items-center flex-1 pl-3 py-1 hover:bg-slate-600 rounded-full"
          >
            <i
              className="rounded-full h-2 mr-2 w-2 inline-block"
              style={{ background: HOST_COLOR }}
            ></i>
            Host
          </Link>
        </div>
        {nodes.map((nodeId) => (
          <div
            key={nodeId}
            className="flex gap-2 items-center bg-slate-700 rounded-full"
          >
            <Link
              href={`/nodes/${nodeId}`}
              className="flex items-center flex-1 pl-3 py-1 hover:bg-slate-600 rounded-full"
            >
              <i
                className="rounded-full h-2 mr-2 w-2 inline-block"
                style={{ background: selectBySeed(COLORS, nodeId) }}
              ></i>
              {nodeId}
            </Link>
            <a
              href={`https://${nodeId}.localhost`}
              className="flex text-gray-400 hover:text-white h-full px-2 items-center rounded-full"
              target="_blank"
              rel="noreferrer"
            >
              <FaWallet />
            </a>
          </div>
        ))}
      </div>
    </div>
  )
}

export default NodesList
