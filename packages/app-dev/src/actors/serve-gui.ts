import { createServer } from "vite"

import { readFileSync } from "node:fs"
import path from "node:path"

import { createActor } from "@dassie/lib-reactive"

import { LOCALHOST } from "../constants/hosts"
import { LOCAL_FOLDER } from "../constants/paths"
import { DEBUG_UI_PORT } from "../constants/ports"
import { server as logger } from "../logger/instances"
import { validateCertificates } from "../utils/validate-certificates"

const debugUiPath = new URL("../../../gui-dev/", import.meta.url).pathname
const certificatePath = path.join(
  LOCAL_FOLDER,
  "tls/localhost/web-localhost.pem",
)
const keyPath = path.join(LOCAL_FOLDER, "tls/localhost/web-localhost-key.pem")

export const DebugUiServerActor = () =>
  createActor(async (sig) => {
    await validateCertificates({
      id: "dev",
      certificates: [
        {
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
        host: LOCALHOST,
        https: {
          cert: readFileSync(certificatePath),
          key: readFileSync(keyPath),
        },
      },
      clearScreen: false,
    })
    await server.listen(DEBUG_UI_PORT)

    logger.info(`listening on https://localhost:${DEBUG_UI_PORT}/`)

    sig.onCleanup(async () => {
      await server.close()
    })
  })
