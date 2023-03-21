import serveStatic from "serve-static"
import { createServer } from "vite"
import type { ViteDevServer } from "vite"

import { readFileSync } from "node:fs"
import { join } from "node:path"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { LOCAL_FOLDER } from "../constants/paths"
import { DEBUG_UI_PORT } from "../constants/ports"
import { validateCertificates } from "./validate-certificates"

const logger = createLogger("das:dev:debug-ui-server")

const debugUiPath = new URL("../../../", import.meta.url).pathname
const certificatePath = join(LOCAL_FOLDER, "tls/localhost/web-localhost.pem")
const keyPath = join(LOCAL_FOLDER, "tls/localhost/web-localhost-key.pem")

const devtoolsServer = () => ({
  name: "html-ext-fallback",
  configureServer(server: ViteDevServer) {
    const devtoolsPath = new URL(
      "../../node_modules/chrome-devtools-frontend-prebuilt/public",
      import.meta.url
    ).pathname

    server.middlewares.use("/devtools", serveStatic(devtoolsPath, {}))
  },
})

export const debugUiServer = () =>
  createActor(async (sig) => {
    await sig.run(validateCertificates, {
      id: "dev",
      certificates: [
        {
          type: "web",
          commonName: "localhost",
          certificatePath,
          keyPath,
        },
      ],
    })

    const server = await createServer({
      root: debugUiPath,
      server: {
        port: DEBUG_UI_PORT,
        https: {
          cert: readFileSync(certificatePath),
          key: readFileSync(keyPath),
        },
      },
      plugins: [devtoolsServer()],
    })
    await server.listen(DEBUG_UI_PORT)

    logger.info(`listening on https://localhost:${DEBUG_UI_PORT}/`)

    sig.onCleanup(async () => {
      await server.close()
    })
  })
