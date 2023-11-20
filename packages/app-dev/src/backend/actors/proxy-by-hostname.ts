import chalk from "chalk"

import { Socket, connect, createServer } from "node:net"

import { createActor } from "@dassie/lib-reactive"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { LOCALHOST } from "../constants/hosts"
import {
  DEBUG_RPC_PORT,
  DEBUG_UI_PORT,
  NODES_START_PORT,
  SNI_PROXY_PORT,
} from "../constants/ports"
import { server as logger } from "../logger/instances"

const HEADER_LENGTH = 5

const CONTENT_TYPE_HANDSHAKE = 22

const EXTENSION_TYPE_SERVER_NAME = 0

const parseServerNameIdentification = (buffer: Buffer): string | undefined => {
  if (buffer.length < HEADER_LENGTH) {
    throw new Error("Initial packet too short to be a TLS message")
  }

  let index = 0

  // TLSPlaintext.type
  // see: https://www.rfc-editor.org/rfc/rfc8446#appendix-B.1
  const contentType = buffer[index++]

  if (contentType !== CONTENT_TYPE_HANDSHAKE) {
    throw new Error("Initial packet is not a TLS handshake message")
  }

  // TLSPlaintext.legacy_record_version
  index += 2

  // TLSPlaintext.length
  const plaintextLength = buffer.readUInt16BE(index)
  index += 2

  if (plaintextLength !== buffer.length - HEADER_LENGTH) {
    throw new Error("Initial packet is not a complete TLS ClientHello")
  }

  // Handshake.type
  // see: https://www.rfc-editor.org/rfc/rfc8446#appendix-B.3
  const handshakeType = buffer[index++]

  if (handshakeType !== 0x01) {
    throw new Error("Initial packet is not a TLS ClientHello")
  }

  // Handshake.length
  index += 3

  // Handshake.legacy_version
  index += 2

  // Handshake.random
  index += 32

  // Handshake.legacy_session_id
  index += 1 + buffer.readUint8(index)

  // Handshake.cipher_suites
  index += 2 + buffer.readUInt16BE(index)

  // Handshake.legacy_compression_methods
  index += 1 + buffer.readUint8(index)

  if (index === buffer.length) {
    return undefined
  }

  // Handshake.extensions
  // Handshake.extensions.length
  const extensionsLength = buffer.readUInt16BE(index)
  index += 2

  const extensionsEnd = index + extensionsLength

  while (index < extensionsEnd) {
    // Extension.extension_type
    const extensionType = buffer.readUInt16BE(index)
    index += 2

    const extensionDataLength = buffer.readUInt16BE(index)
    index += 2

    if (extensionType !== EXTENSION_TYPE_SERVER_NAME) {
      index += extensionDataLength
      continue
    }

    // ServerNameList
    // see: https://www.rfc-editor.org/rfc/rfc6066#section-3
    const serverNameListLength = buffer.readUInt16BE(index)
    index += 2

    for (
      let serverNameIndex = 0;
      serverNameIndex < serverNameListLength;
      serverNameIndex++
    ) {
      // ServerNameList[i].name_type
      const nameType = buffer.readUInt8(index)
      index += 1

      // ServerNameList[i].length
      const nameLength = buffer.readUInt16BE(index)
      index += 2

      if (nameType !== 0) {
        index += nameLength
        continue
      }

      // ServerNameList[i].name
      return buffer.toString("utf8", index, index + nameLength)
    }
  }

  return undefined
}

const createProxy = async (port: number) => {
  const clientSocket = connect(port, LOCALHOST)

  return new Promise<Socket>((resolve, reject) => {
    clientSocket.on("connect", () => {
      clientSocket.off("error", reject)
      resolve(clientSocket)
    })
    clientSocket.on("error", reject)
  })
}

const HOST_REGEX = /^d(\d+).localhost$/

const getPortByHostname = (host: string | undefined) => {
  if (typeof host === "string") {
    if (host === "dev-rpc.localhost") {
      return DEBUG_RPC_PORT
    }

    const match = HOST_REGEX.exec(host)
    if (match) {
      return NODES_START_PORT + (Number(match[1]!) - 1)
    }
  }

  return DEBUG_UI_PORT
}

const handleIncomingConnection = async (socket: Socket) => {
  socket.on("error", (error: unknown) => {
    logger.error("error on incoming socket", { error })
  })

  const firstPacket = await new Promise<Buffer>((resolve, reject) => {
    socket.once("data", (data) => {
      socket.off("error", reject)
      resolve(data)
    })
    socket.on("error", reject)
  })

  const host = parseServerNameIdentification(firstPacket)

  const port = getPortByHostname(host)

  const proxy = await createProxy(port)
  proxy.on("error", (error: unknown) => {
    logger.error("error on proxy socket", { error })
  })
  proxy.write(firstPacket)
  socket.pipe(proxy).pipe(socket)
}

export const ProxyByHostnameActor = () =>
  createActor((sig) => {
    const connections = new Set<Socket>()

    const server = createServer((socket) => {
      connections.add(socket)

      handleIncomingConnection(socket).catch((error: unknown) => {
        socket.destroy()
        if (isErrorWithCode(error, "ECONNREFUSED")) {
          logger.warn(
            "proxy connection refused, maybe the node is still starting up",
            { port: (error as unknown as { port: number }).port },
          )
          return
        }
        logger.error("failed to proxy incoming connection", { error })
      })

      socket.on("close", () => {
        connections.delete(socket)
      })
    })

    server.on("error", (error: unknown) => {
      if (isErrorWithCode(error, "EACCES")) {
        console.warn(
          "Unable to bind to port 443. Try running `sudo sysctl -w net.ipv4.ip_unprivileged_port_start=0`.",
        )
        return
      }

      logger.error("sni proxy error", { error })
    })

    server.listen(SNI_PROXY_PORT)

    // eslint-disable-next-line no-console
    console.log(
      `  ${chalk.bold("Debug UI:")} https://localhost/ ${chalk.dim(
        "<-- Start here",
      )}\n`,
    )

    sig.onCleanup(async () => {
      await new Promise<void>((resolve, reject) => {
        for (const connection of connections) {
          connection.destroy()
        }

        server.close((error: unknown) => {
          if (error) reject(error)
          else resolve()
        })
      })
    })
  })
