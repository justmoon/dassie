import { ViteDevServer } from "vite"

import { createAbstract } from "@dassie/lib-reactive"

export const ViteServer = () => createAbstract<ViteDevServer>()
