import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { BEACON_COUNT } from "../constants/development-beacons"
import { viteNodeServerValue } from "../services/vite-node-server"
import { viteServerValue } from "../services/vite-server"
import { generateBeaconConfig } from "../utils/generate-beacon-config"
import { fileChangeTopic } from "./handle-file-change"
import { runBeaconChildProcess } from "./run-beacon-child-process"
import { CertificateInfo, validateCertificates } from "./validate-certificates"

const logger = createLogger("das:dev:run-beacons")

export interface BeaconDefinition<T> {
  id: string
  port: number
  config: T
  url: string
  entry?: string
}

export const runBeacons = async (sig: EffectContext) => {
  const viteServer = await sig.get(viteServerValue)
  const nodeServer = await sig.get(viteNodeServerValue)

  logger.debug("starting beacon processes")

  await Promise.all(
    Array.from({ length: BEACON_COUNT })
      .fill(null)
      .map((_, index) =>
        sig.run(async () => {
          const beacon = generateBeaconConfig(index)

          // Restart child processes when a file changes
          sig.subscribe(fileChangeTopic)

          const neededCertificates: CertificateInfo[] = [
            {
              type: "web",
              certificatePath: beacon.config.tlsWebCertFile,
              keyPath: beacon.config.tlsWebKeyFile,
            },
          ]
          await sig.run(validateCertificates, {
            id: beacon.id,
            certificates: neededCertificates,
          })

          await sig.run(runBeaconChildProcess, {
            viteServer,
            nodeServer,
            beacon,
          })
        })
      )
  )
}
