import type { BootstrapNodesConfig } from "../config/environment-config"
import type { NodeId } from "../peer-protocol/types/node-id"

export const DEFAULT_BOOTSTRAP_NODES: BootstrapNodesConfig = [
  {
    id: "dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW" as NodeId,
    url: "https://test1.dassie.xyz",
    publicKey: "QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM",
  },
  {
    id: "dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U" as NodeId,
    url: "https://test2.dassie.xyz",
    publicKey: "tmS1PB42DEjYi-Ua4RNguhweId9YjT9Kml3uq3iZSuQ",
  },
]
