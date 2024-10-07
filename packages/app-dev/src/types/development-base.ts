import type { ViteDevServer } from "vite"
import { ViteNodeServer as ViteNodeServerType } from "vite-node/server"

import type { ActorContext, Reactor } from "@dassie/lib-reactive"

export interface DevelopmentBase {
  readonly viteServer: ViteDevServer
  readonly viteNodeServer: ViteNodeServerType
  readonly restart: () => void
  readonly isFirst: boolean
}

export type DevelopmentReactor = Reactor<DevelopmentBase>
export type DevelopmentActorContext = ActorContext<DevelopmentBase>
