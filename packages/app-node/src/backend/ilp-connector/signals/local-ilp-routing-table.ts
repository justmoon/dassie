import { createSignal } from "@dassie/lib-reactive"

import type { IlpDestinationInfo } from "../send-outgoing-packets"
import PrefixMap from "../utils/prefix-map"

export const localIlpRoutingTableSignal = () =>
  createSignal(new PrefixMap<IlpDestinationInfo>())
