import { readFile, unlink } from "node:fs/promises"

import { DASSIE_DATABASE_SCHEMA } from "@dassie/app-node/src/backend/database/schema"
import { SettlementSchemeId } from "@dassie/app-node/src/backend/peer-protocol/types/settlement-scheme-id"
import { createDatabase } from "@dassie/lib-sqlite"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { DEBUG_UI_PORT } from "../constants/ports"
import {
  NodeConfig,
  generatePeerInfo,
  nodeIndexToPublicKey,
} from "./generate-node-config"

export const prefillDatabase = async ({
  id,
  hostname,
  httpsPort,
  dataPath,
  dassieNodeKey,
  tlsWebCertFile,
  tlsWebKeyFile,
  sessionToken,
  peers,
}: NodeConfig) => {
  const databasePath = `${dataPath}/dassie.sqlite3`

  // Delete the database if it already exists.
  try {
    await unlink(databasePath)
  } catch (error) {
    if (!isErrorWithCode(error, "ENOENT")) {
      throw error
    }
  }

  const database = createDatabase({
    path: databasePath,
    schema: DASSIE_DATABASE_SCHEMA,
  })

  database.scalars.configRealm.set("test")

  database.scalars.configHostname.set(hostname)
  database.scalars.configHttpsPort.set(httpsPort)
  database.scalars.configEnableHttpServer.set(false)

  database.scalars.configAlias.set(id)

  database.scalars.configDassieKey.set(dassieNodeKey)
  database.scalars.configTlsWebCert.set(await readFile(tlsWebCertFile, "utf8"))
  database.scalars.configTlsWebKey.set(await readFile(tlsWebKeyFile, "utf8"))
  database.scalars.configExchangeRateUrl.set(
    `https://localhost:${DEBUG_UI_PORT}/rates.json`,
  )

  database.tables.settlementSchemes.insertOne({
    id: "stub" as SettlementSchemeId,
    config: "{}",
  })

  for (const peer of peers) {
    const peerInfo = generatePeerInfo(peer)
    const publicKey = nodeIndexToPublicKey(peer)

    const { lastInsertRowid: nodeRowid } = database.tables.nodes.insertOne({
      id: peerInfo.nodeId,
      public_key: Buffer.from(publicKey),
      url: peerInfo.url,
      alias: peerInfo.alias,
    })

    database.tables.peers.insertOne({
      node: nodeRowid,
      settlement_scheme_id: "stub" as SettlementSchemeId,
    })
  }

  database.tables.sessions.insertOne({
    token: sessionToken,
  })

  database.raw.close()
}
