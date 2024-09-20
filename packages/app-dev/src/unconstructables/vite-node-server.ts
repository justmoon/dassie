import type { ViteNodeServer as ViteNodeServerType } from "vite-node/server"

import { createAbstract } from "@dassie/lib-reactive"

export const ViteNodeServer = () => createAbstract<ViteNodeServerType>()
