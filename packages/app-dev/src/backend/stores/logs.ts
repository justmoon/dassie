import { createStore } from "@xen-ilp/lib-reactive"

import type { NodeLogLine } from "../topics/log-message"

export const logsStore = createStore<NodeLogLine[]>("logs", [])
