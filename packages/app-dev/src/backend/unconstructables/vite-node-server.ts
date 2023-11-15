import type { ViteNodeServer as ViteNodeServerType } from "vite-node/server"

import { createUnconstructable } from "@dassie/lib-reactive"

export const ViteNodeServer = () => createUnconstructable<ViteNodeServerType>()
