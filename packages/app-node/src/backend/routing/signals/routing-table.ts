import { createSignal } from "@dassie/lib-reactive"

import type { IlpDestinationInfo } from "../../ilp-connector/functions/send-packet"
import PrefixMap from "../utils/prefix-map"

export const routingTableSignal = () =>
  createSignal(new PrefixMap<IlpDestinationInfo>())
