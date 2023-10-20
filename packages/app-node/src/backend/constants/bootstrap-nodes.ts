import type { BootstrapNodesConfig } from "../config/environment-config"
import type { NodeId } from "../peer-protocol/types/node-id"

export const DEFAULT_BOOTSTRAP_NODES: BootstrapNodesConfig = [
  {
    nodeId: "NKbwOMCw0DTToPM4tyyVASWxM4NP3MKW-W76gu-cpz8" as NodeId,
    url: "https://test1.dassie.xyz",
    alias: "test1.dassie.xyz",
    nodePublicKey: "QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM",
  },
  {
    nodeId: "yrjUANXiJ5S6JXxwo7jdKKJjdLtK129UKkpS2HlGFMY" as NodeId,
    url: "https://test2.dassie.xyz",
    alias: "test2.dassie.xyz",
    nodePublicKey: "tmS1PB42DEjYi-Ua4RNguhweId9YjT9Kml3uq3iZSuQ",
  },
]
