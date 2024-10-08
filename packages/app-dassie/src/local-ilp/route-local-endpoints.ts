import { createActor, watchStoreChanges } from "@dassie/lib-reactive"

import { NodeIlpAddressSignal } from "../ilp-connector/computed/node-ilp-address"
import { RoutingTableSignal } from "../routing/signals/routing-table"
import { LocalEndpointsStore } from "./stores/local-endpoints"

export const RouteLocalEndpointsActor = () =>
  createActor((sig) => {
    const nodeIlpAddress = sig.readAndTrack(NodeIlpAddressSignal)
    const localEndpointsStore = sig.reactor.use(LocalEndpointsStore)
    const routingTable = sig.reactor.use(RoutingTableSignal)

    const activeEndpoints = new Set<string>()

    for (const localEndpointInfo of localEndpointsStore.read().values()) {
      routingTable
        .read()
        .set(`${nodeIlpAddress}.${localEndpointInfo.localIlpAddressPart}`, {
          type: "fixed",
          destination: localEndpointInfo,
        })
      activeEndpoints.add(localEndpointInfo.localIlpAddressPart)
    }

    watchStoreChanges(sig, localEndpointsStore, {
      registerLocalEndpoint: (localEndpointInfo) => {
        routingTable
          .read()
          .set(`${nodeIlpAddress}.${localEndpointInfo.localIlpAddressPart}`, {
            type: "fixed",
            destination: localEndpointInfo,
          })
        activeEndpoints.add(localEndpointInfo.localIlpAddressPart)
      },
      unregisterLocalEndpoint: (localAddressPart) => {
        routingTable.read().delete(`${nodeIlpAddress}.${localAddressPart}`)
        activeEndpoints.delete(localAddressPart)
      },
      setHandler: () => {
        // no-op
      },
    })

    sig.onCleanup(() => {
      for (const localAddressPart of activeEndpoints) {
        routingTable.read().delete(`${nodeIlpAddress}.${localAddressPart}`)
      }
    })
  })
