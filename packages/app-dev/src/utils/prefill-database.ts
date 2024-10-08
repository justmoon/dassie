import { readFile, unlink } from "node:fs/promises"

import type { BtpToken } from "@dassie/app-dassie/src/api-keys/types/btp-token"
import { DASSIE_DATABASE_SCHEMA } from "@dassie/app-dassie/src/database/schema"
import type { NodeId } from "@dassie/app-dassie/src/peer-protocol/types/node-id"
import type { SettlementSchemeId } from "@dassie/app-dassie/src/peer-protocol/types/settlement-scheme-id"
import { createDatabase } from "@dassie/lib-sqlite"
import { isErrorWithCode } from "@dassie/lib-type-utils"
import { serializeEd25519Key } from "@dassie/lib-x509"

import { DEBUG_UI_PORT } from "../constants/ports"
import { setup as logger } from "../logger/instances"
import { type NodeConfig, generatePeerInfo } from "./generate-node-config"

interface BasicNodeInfo {
  id: NodeId
  publicKey: Uint8Array
  url: string
  alias: string
}

export const prefillDatabase = async ({
  id,
  fullId,
  hostname,
  httpsPort,
  dataPath,
  dassieNodeKey,
  tlsWebCertFile,
  tlsWebKeyFile,
  sessionToken,
  bootstrapNodes,
  registeredNodes,
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

  function insertNodeToNodeTable(node: BasicNodeInfo) {
    let rowid = database.tables.nodes.selectFirst({ id: node.id })?.rowid

    if (!rowid) {
      rowid = database.tables.nodes.insertOne({
        id: node.id,
        public_key: Buffer.from(node.publicKey),
        url: node.url,
        alias: node.alias,
      }).lastInsertRowid
    }

    return rowid
  }

  if (
    bootstrapNodes.some(({ id: bootstrapNodeId }) => bootstrapNodeId === fullId)
  ) {
    for (const node of registeredNodes) {
      const nodeRowid = insertNodeToNodeTable(node)

      database.tables.registrations.insertOne({
        node: nodeRowid,
        registered_at: BigInt(Date.now()),
        renewed_at: BigInt(Date.now()),
      })
    }
  }

  for (const peer of peers) {
    const peerInfo = generatePeerInfo(peer)

    const { lastInsertRowid: nodeRowid } = database.tables.nodes.insertOne(
      {
        id: peerInfo.id,
        public_key: Buffer.from(peerInfo.publicKey),
        url: peerInfo.url,
        alias: peerInfo.alias,
      },
      { ignoreConflicts: true },
    )

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
