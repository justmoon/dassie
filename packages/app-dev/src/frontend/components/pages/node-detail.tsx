import * as Tabs from "@radix-ui/react-tabs"
import { useMemo, useState } from "react"
import { FaLink } from "react-icons/fa"
import { format } from "timeago.js"
import { Link } from "wouter"

import { selectBySeed } from "@dassie/lib-logger"

import { convertVanityNodeIdToFriendly } from "../../../common/utils/vanity-node-id-to-friendly"
import { COLORS } from "../../constants/palette"
import {
  type FirehoseEvent,
  useNodeFirehose,
} from "../../hooks/use-node-firehose"
import { useNodeRemoteSignal } from "../../hooks/use-node-state"
import LogViewer from "../log-viewer/log-viewer"

interface BasicNodeElementProperties {
  nodeId: string
}
const NodeHeader = ({ nodeId }: BasicNodeElementProperties) => {
  const color = useMemo(() => selectBySeed(COLORS, nodeId), [nodeId])
  return (
    <header>
      <div className="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
        <h1 className="font-bold leading-tight text-3xl text-gray-100">
          <i
            className="rounded-full h-5 mr-4 w-5 inline-block"
            style={{ background: color }}
          ></i>
          Node:{" "}
          <span
            style={{
              color: color,
            }}
          >
            {nodeId}
          </span>
          <a
            href={`https://${nodeId}.localhost`}
            target="_blank"
            className="ml-8 text-gray text-lg"
            rel="noreferrer"
          >
            Wallet
            <FaLink className="inline-block ml-2" />
          </a>
        </h1>
      </div>
    </header>
  )
}

const NodeLink = ({ nodeId }: BasicNodeElementProperties) => {
  const friendlyId = convertVanityNodeIdToFriendly(nodeId)
  return (
    <Link
      href={`/nodes/${friendlyId}`}
      className="font-bold"
      style={{ color: selectBySeed(COLORS, friendlyId) }}
    >
      {friendlyId}
    </Link>
  )
}

const NodeTable = ({ nodeId }: BasicNodeElementProperties) => {
  const nodeTable = useNodeRemoteSignal(nodeId, "nodeTable")
  const routingTable = useNodeRemoteSignal(nodeId, "routingTable")

  if (!nodeTable.data || !routingTable.data) return null

  const sortedNodeTable = [...nodeTable.data.values()]
    .map((node) => {
      const routingTableEntry = routingTable.data.get(node.nodeId)

      return {
        ...node,
        distance: routingTableEntry?.distance ?? Number.POSITIVE_INFINITY,
        nextHopNodes: routingTable.data.get(node.nodeId)?.firstHopOptions ?? [],
      }
    })
    // Sort first by distance, then by nodeId
    .sort((a, b) => {
      if (a.distance === b.distance) {
        return a.nodeId.localeCompare(b.nodeId)
      }

      return a.distance - b.distance
    })

  return (
    <div className="min-h-0">
      <h2 className="font-bold text-xl">Node Table</h2>
      <table className="border-separate border-spacing-2 my-4 -ml-2">
        <thead>
          <tr>
            <th className="text-left">Subnet</th>
            <th className="text-left">Node</th>
            <th>Distance</th>
            <th className="text-left">Peering</th>
            <th>Next Hop Options</th>
            <th>Neighbors</th>
            <th className="text-left">Last Seen</th>
          </tr>
        </thead>
        <tbody>
          {sortedNodeTable.map((node) => {
            return (
              <tr key={node.nodeId}>
                <td className="text-slate-400">{node.subnetId}</td>
                <td className="">
                  <NodeLink nodeId={node.nodeId} />
                </td>
                <td>{node.distance}</td>
                <td>{node.peerState.id}</td>
                <td>
                  <div className="flex gap-2">
                    {node.nextHopNodes.map((nodeId) => (
                      <NodeLink key={nodeId} nodeId={nodeId} />
                    ))}
                  </div>
                </td>
                <td>
                  <div className="flex gap-2">
                    {node.linkState.neighbors.map((nodeId) => (
                      <NodeLink key={nodeId} nodeId={nodeId} />
                    ))}
                  </div>
                </td>
                {node.peerState.id === "none" ? (
                  <td></td>
                ) : (
                  <td
                    className=""
                    title={new Date(node.peerState.lastSeen).toISOString()}
                  >
                    {format(new Date(node.peerState.lastSeen))}
                  </td>
                )}
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

interface NodeFirehoseEventListProperties {
  messageId: number | undefined
  events: FirehoseEvent[]
  onClick: (messageId: number) => void
}

const NodeFirehoseEventList = ({
  events,
  onClick,
}: NodeFirehoseEventListProperties) => {
  return (
    <div className="h-full overflow-y-auto">
      <table className="border-separate border-spacing-2 -m-2">
        <thead className="relative">
          <tr>
            <th className="text-left">Topic</th>
            <th className="text-left">Message</th>
          </tr>
        </thead>
        <tbody>
          {events.map(({ topic }, index) => (
            <tr key={index} onClick={() => onClick(index)}>
              <td className="">{topic}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

interface NodeFirehoseEventDetailProperties {
  messageId: number
  events: FirehoseEvent[]
}

const NodeFirehoseEventDetail = ({
  messageId,
  events,
}: NodeFirehoseEventDetailProperties) => {
  return (
    <div className="p-4 overflow-auto">
      <pre>{events[messageId]?.message}</pre>
    </div>
  )
}

interface NodeFirehoseViewerProperties extends BasicNodeElementProperties {
  events: FirehoseEvent[]
}

const NodeFirehoseViewer = ({ events }: NodeFirehoseViewerProperties) => {
  const [messageId, setMessageId] = useState<number | undefined>(undefined)
  return (
    <div className="h-full grid p-4 gap-4 grid-cols-[400px_auto]">
      <NodeFirehoseEventList
        messageId={messageId}
        events={events}
        onClick={(messageId) => setMessageId(messageId)}
      />
      <div>
        {messageId == undefined ? null : (
          <NodeFirehoseEventDetail messageId={messageId} events={events} />
        )}
      </div>
    </div>
  )
}

const NodeLogViewer = ({ nodeId }: BasicNodeElementProperties) => {
  return <LogViewer filter={({ node }) => node === nodeId} />
}

const NodeDetail = ({ nodeId }: BasicNodeElementProperties) => {
  // We are tracking events from this component so that events continue to be captured as the user browses different tabs.
  const events = useNodeFirehose(nodeId)

  return (
    <div className="h-screen grid grid-rows-[min-content_auto] py-10">
      <NodeHeader nodeId={nodeId} />
      <Tabs.Root
        defaultValue="logs"
        className="mx-auto w-full min-h-0 max-w-7xl grid grid-rows-[min-content_auto] pt-8 gap-4 sm:px-6 lg:px-8"
      >
        <Tabs.List className="flex flex-wrap -mb-px">
          <Tabs.Trigger
            value="logs"
            className="border-transparent rounded-t-lg border-b-2 p-4 inline-block hover:border-gray-300 hover:text-gray-300"
          >
            Logs
          </Tabs.Trigger>
          <Tabs.Trigger
            value="state"
            className="border-transparent rounded-t-lg border-b-2 p-4 inline-block hover:border-gray-300 hover:text-gray-300"
          >
            State
          </Tabs.Trigger>
          <Tabs.Trigger
            value="events"
            className="border-transparent rounded-t-lg border-b-2 p-4 inline-block hover:border-gray-300 hover:text-gray-300"
          >
            Events
          </Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content
          value="logs"
          className="rounded-lg bg-gray-800 min-h-0 p-4"
        >
          <NodeLogViewer nodeId={nodeId} />
        </Tabs.Content>
        <Tabs.Content
          value="state"
          className="rounded-lg bg-gray-800 min-h-0 p-4"
        >
          <NodeTable nodeId={nodeId} />
        </Tabs.Content>
        <Tabs.Content value="events" className="rounded-lg bg-gray-800 min-h-0">
          <NodeFirehoseViewer nodeId={nodeId} events={events} />
        </Tabs.Content>
      </Tabs.Root>
    </div>
  )
}

export default NodeDetail
