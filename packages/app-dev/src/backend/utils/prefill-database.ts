import { readFile, unlink } from "node:fs/promises"

import { DASSIE_DATABASE_SCHEMA } from "@dassie/app-node/src/backend/database/open-database"
import { SettlementSchemeId } from "@dassie/app-node/src/backend/peer-protocol/types/settlement-scheme-id"
import { createDatabase } from "@dassie/lib-sqlite"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { DEBUG_UI_PORT } from "../constants/ports"
import { NodeConfig, generatePeerInfo } from "./generate-node-config"

export const prefillDatabase = async ({
  id,
  hostname,
  port,
  dataPath,
  tlsDassieCertFile,
  tlsDassieKeyFile,
  tlsWebCertFile,
  tlsWebKeyFile,
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

  database.scalars.set("config.realm", "test")

  database.scalars.set("config.hostname", hostname)
  database.scalars.set("config.port", port)

  database.scalars.set("config.alias", id)

  database.scalars.set(
    "config.tls_dassie_cert",
    await readFile(tlsDassieCertFile, "utf8")
  )
  database.scalars.set(
    "config.tls_dassie_key",
    await readFile(tlsDassieKeyFile, "utf8")
  )
  database.scalars.set(
    "config.tls_web_cert",
    await readFile(tlsWebCertFile, "utf8")
  )
  database.scalars.set(
    "config.tls_web_key",
    await readFile(tlsWebKeyFile, "utf8")
  )
  database.scalars.set(
    "config.exchange_rate_url",
    `https://localhost:${DEBUG_UI_PORT}/rates.json`
  )

  database.tables.settlementSchemes.insertOne({
    id: "stub" as SettlementSchemeId,
    config: {},
  })

  const peersInfo = peers.map((peerIndex) => generatePeerInfo(peerIndex))

  for (const peer of peersInfo) {
    const { rowid: nodeRowid } = database.tables.nodes.insertOne({
      id: peer.nodeId,
      public_key: peer.nodePublicKey,
      url: peer.url,
      alias: peer.alias,
    })

    database.tables.peers.insertOne({
      node: nodeRowid,
      settlement_scheme_id: "stub" as SettlementSchemeId,
    })
  }

  database.raw.close()
}
