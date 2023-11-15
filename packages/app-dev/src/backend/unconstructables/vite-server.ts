import { ViteDevServer } from "vite"

import { createUnconstructable } from "@dassie/lib-reactive"

export const ViteServer = () => createUnconstructable<ViteDevServer>()
