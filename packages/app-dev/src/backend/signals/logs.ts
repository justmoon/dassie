import type { IndexedLogLine } from "../features/logs"
import { createImmerStore } from "../utils/immer-store"

export const logsStore = () => createImmerStore<IndexedLogLine[]>([])
