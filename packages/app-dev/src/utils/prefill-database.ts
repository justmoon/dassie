import { readFile, unlink } from "node:fs/promises"

import type { BtpToken } from "@dassie/app-dassie/src/api-keys/types/btp-token"
import { DASSIE_DATABASE_SCHEMA } from "@dassie/app-dassie/src/database/schema"
import type { SettlementSchemeId } from "@dassie/app-dassie/src/peer-protocol/types/settlement-scheme-id"
import { createDatabase } from "@dassie/lib-sqlite"
import { isErrorWithCode } from "@dassie/lib-type-utils"
import { serializeEd25519Key } from "@dassie/lib-x509"

import { DEBUG_UI_PORT } from "../constants/ports"
import { setup as logger } from "../logger/instances"
import { type NodeConfig, generatePeerInfo } from "./generate-node-config"

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
  settlementMethods,
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
    logger,
  })

  database.scalars.configRealm.set("test")

  database.scalars.configHostname.set(hostname)
  database.scalars.configHttpsPort.set(httpsPort)
  database.scalars.configEnableHttpServer.set(false)

  database.scalars.configAlias.set(id)

  database.scalars.configDassieKey.set(
    serializeEd25519Key(dassieNodeKey, "private"),
  )
  database.scalars.configTlsWebCert.set(await readFile(tlsWebCertFile, "utf8"))
  database.scalars.configTlsWebKey.set(await readFile(tlsWebKeyFile, "utf8"))
  database.scalars.configExchangeRateUrl.set(
    `https://localhost:${DEBUG_UI_PORT}/rates.json`,
  )

  for (const method of settlementMethods) {
    database.tables.settlementSchemes.insertOne({
      id: method as SettlementSchemeId,
      config: "{}",
    })
  }

  for (const peer of peers) {
    const peerInfo = generatePeerInfo(peer)

    const { lastInsertRowid: nodeRowid } = database.tables.nodes.insertOne({
      id: peerInfo.nodeId,
      public_key: Buffer.from(peerInfo.nodePublicKey),
      url: peerInfo.url,
      alias: peerInfo.alias,
    })

    database.tables.peers.insertOne({
      node: nodeRowid,
      settlement_scheme_id: peerInfo.settlement.settlementSchemeId,
      settlement_scheme_state: JSON.stringify(
        peerInfo.settlement.settlementSchemeState,
      ),
    })
  }

  database.tables.sessions.insertOne({
    token: sessionToken,
  })

  database.tables.btpTokens.insertOne({
    token: "beNrVTiQLKReCr9d-YBPjA" as BtpToken,
  })

  database.raw.close()
}
