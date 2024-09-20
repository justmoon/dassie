import { createSignal } from "@dassie/lib-reactive"

import type { NodeId } from "../types/node-id"

/**
 * This signal contains a map of node IDs of bootstrap nodes and their last known node list hash.
 */
export const BootstrapNodeListHashesSignal = () =>
  createSignal(new Map<NodeId, Uint8Array>())
