// Generates the golden vectors for the core crates from the original Dassie
// implementation at commit e962ab78. See README.md in this directory for how
// to run it. It is written as a Vitest file because that is the only runner
// in the original's tree that resolves its extensionless TypeScript imports.
//
// Every vector here is produced by the original's own code (or copied from
// its fixtures), so a mismatch in the Rust tests means the rewrite differs
// from the original. Where the original is wrong and the rewrite fixes it, the
// Rust tests hold a hand-written vector instead, marked with the gap ID.

import { ed25519, x25519 } from "@noble/curves/ed25519.js"
import { getPublicKey, signAsync } from "./crypto/ed25519"
import { createHash, createHmac } from "node:crypto"
import { mkdirSync, writeFileSync } from "node:fs"
import path from "node:path"
import { base64ToUint8Array, stringToUint8Array } from "uint8array-extras"
import { test } from "vitest"
import { Wallet, encodeSeed } from "xrpl"

import {
  integerAsBigint,
  objectIdentifier,
  octetString,
  sequenceOf,
  utf8String,
} from "@dassie/lib-oer"
import {
  ILDCP_ADDRESS,
  ILDCP_CONDITION,
  serializeIldcpResponse,
} from "@dassie/lib-protocol-ildcp"
import {
  IlpType,
  serializeAmountTooLargeData,
  serializeIlpPacket,
  timestampToInterledgerTime,
} from "@dassie/lib-protocol-ilp"
import {
  BtpContentType,
  generateBtpError,
  generateBtpMessage,
  generateBtpResponse,
  generateBtpTransfer,
} from "@dassie/lib-protocol-utils"
import { createMockDeterministicCrypto } from "@dassie/lib-reactive"
import { createCrypto } from "@dassie/lib-reactive-io"

import {
  TEST_NODE_VANITY_ENTROPY,
  TEST_NODE_VANITY_IDS,
  TEST_NODE_VANITY_MNEMONICS,
  TEST_NODE_VANITY_SEEDS,
} from "../../app-dev/src/constants/vanity-nodes"
import ildcpFixtures from "../../lib-protocol-ildcp/src/test/fixtures/packets"
import ilpFixtures from "../../lib-protocol-ilp/src/test/fixtures/packets"
import { getPskEnvironment } from "../../lib-protocol-stream/src/crypto/functions"
import streamFixtures from "../../lib-protocol-stream/src/test/fixtures/packets"
import { serializePacket } from "../../lib-protocol-stream/src/packets/parser"
import { getSampleObjectIdentifiers } from "../../lib-oer/test/utils/sample-object-identifiers"
import { getVariableLengthSamples } from "../../lib-oer/test/utils/sample-numbers"
import { utf8TestValues } from "../../lib-oer/test/utils/sample-strings"
import {
  dassieNodeFingerprintV1Id,
  dassiePeerNodeInfoId,
} from "./constants/object-identifiers"
import { getPrivateSeedAtPath, calculatePathHmac } from "./crypto/utils/seed-paths"
import { calculateNodeId } from "./ilp-connector/utils/calculate-node-id"
import {
  nodeIdSchema,
  peerMessage,
  peerNodeInfo,
  signedPeerNodeInfo,
} from "./peer-protocol/peer-schema"
import { calculateMessageHmac } from "./peer-protocol/utils/calculate-message-hmac"

const SOURCE = "e962ab78"
const OUT = process.env["VECTORS_OUT"]

const hex = (bytes: Uint8Array) => Buffer.from(bytes).toString("hex")
const fromHex = (value: string) => new Uint8Array(Buffer.from(value, "hex"))

// JSON has no bigint and no byte arrays; vectors carry them as decimal
// strings and lowercase hex respectively.
const toJson = (value: unknown): unknown => {
  if (typeof value === "bigint") return value.toString()
  if (value instanceof Uint8Array) return hex(value)
  if (Array.isArray(value)) return value.map((item) => toJson(item))
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, toJson(item)]),
    )
  }
  return value
}

const write = (relativePath: string, vectors: Record<string, unknown>) => {
  if (!OUT) throw new Error("Set VECTORS_OUT to the rewrite's crates/ directory")
  const file = path.join(OUT, relativePath)
  mkdirSync(path.dirname(file), { recursive: true })
  const document = { source: SOURCE, ...vectors }
  writeFileSync(file, JSON.stringify(toJson(document), undefined, 2) + "\n")
}

const xrplWallet = (entropy: Uint8Array) => {
  const familySeed = encodeSeed(entropy.subarray(0, 16), "ed25519")
  const wallet = Wallet.fromSeed(familySeed)
  return { familySeed, address: wallet.classicAddress, publicKey: wallet.publicKey }
}

test("identity vectors", async () => {
  // A deterministic spread over the vanity table, plus the last entry (the
  // record holder), keeps the file small while covering many IDs.
  // The ID list is an array in key order, so it is indexed by position, not
  // by key; the record holder's key is far beyond the array's length.
  const entries = Object.keys(TEST_NODE_VANITY_ENTROPY)
    .map((key, position) => ({ index: Number(key), position }))
    .filter(({ index }, position, all) => index % 16 === 0 || position === all.length - 1)

  const chain = entries.map(({ index, position }) => {
    const masterSeed = fromHex(TEST_NODE_VANITY_SEEDS[index]!)
    const nodeKey = calculatePathHmac(Buffer.from(masterSeed), "node")
    const publicKey = getPublicKey(nodeKey)
    const testnetEntropy = getPrivateSeedAtPath(
      nodeKey,
      "node/settlement-module/xrpl-testnet/seed-testnet" as never,
    )
    const mainnetEntropy = getPrivateSeedAtPath(
      nodeKey,
      "node/settlement-module/xrpl/seed-mainnet" as never,
    )
    return {
      index,
      entropy: TEST_NODE_VANITY_ENTROPY[index],
      mnemonic: TEST_NODE_VANITY_MNEMONICS[index],
      masterSeed,
      nodeKey,
      publicKey,
      nodeId: calculateNodeId(publicKey),
      expectedNodeId: TEST_NODE_VANITY_IDS[position],
      loginSecret: getPrivateSeedAtPath(nodeKey, "node/login" as never),
      devSessionToken: calculatePathHmac(Buffer.from(masterSeed), "dev/session"),
      xrplTestnetEntropy: testnetEntropy,
      xrplTestnetWallet: xrplWallet(testnetEntropy),
      xrplMainnetEntropy: mainnetEntropy,
      xrplMainnetWallet: xrplWallet(mainnetEntropy),
    }
  })

  for (const entry of chain) {
    if (entry.nodeId !== entry.expectedNodeId) {
      throw new Error(`Vanity table mismatch at ${entry.index}`)
    }
  }

  write("dassie-identity/tests/data/chain.json", { chain })

  const seedA = new Uint8Array(32).fill(1)
  const seedB = new Uint8Array(32).fill(3)
  const sessionPrivate = new Uint8Array(32).fill(2)
  const publicA = getPublicKey(seedA)
  const publicB = getPublicKey(seedB)
  const sharedSecret = x25519.getSharedSecret(
    sessionPrivate,
    ed25519.utils.toMontgomery(publicB),
  )

  const bootstrap = [
    ["test1.dassie.xyz", "QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM"],
    ["test2.dassie.xyz", "tmS1PB42DEjYi-Ua4RNguhweId9YjT9Kml3uq3iZSuQ"],
  ].map(([host, key]) => {
    const publicKey = new Uint8Array(Buffer.from(key!, "base64url"))
    return { host, publicKey, nodeId: calculateNodeId(publicKey) }
  })

  const keys = await Promise.all(
    [seedA, seedB].map(async (seed) => {
      const publicKey = getPublicKey(seed)
      const message = stringToUint8Array("dassie test message")
      return {
        seed,
        publicKey,
        nodeId: calculateNodeId(publicKey),
        x25519Secret: ed25519.utils.toMontgomerySecret(seed),
        x25519Public: ed25519.utils.toMontgomery(publicKey),
        message,
        signature: await signAsync(message, seed),
      }
    }),
  )

  write("dassie-identity/tests/data/keys.json", {
    bootstrap,
    keys,
    session: {
      privateKey: sessionPrivate,
      publicKey: x25519.getPublicKey(sessionPrivate),
      peerPublicKey: publicB,
      sharedSecret,
    },
    nodeIdHashInput: {
      oid: dassieNodeFingerprintV1Id,
      publicKey: publicA,
      encoded: objectIdentifier().serializeOrThrow(dassieNodeFingerprintV1Id),
    },
  })
})

test("oer vectors", async () => {
  const unbounded = integerAsBigint([0n, undefined])
  const variableUnsigned = getVariableLengthSamples(false).map(([value]) => ({
    value,
    encoded: unbounded.serializeOrThrow(value),
  }))

  const objectIdentifiers = getSampleObjectIdentifiers().map(([value]) => ({
    value,
    encoded: objectIdentifier().serializeOrThrow(value),
  }))

  const utf8 = utf8TestValues.map(([value]) => ({
    value,
    encoded: utf8String().serializeOrThrow(value),
  }))

  const octetLengths = [0, 1, 127, 128, 255, 256, 65535, 65536].map((length) => {
    const encoded = octetString().serializeOrThrow(new Uint8Array(length))
    // Only the length determinant is interesting; the zero bytes are not.
    return { length, prefix: encoded.subarray(0, encoded.length - length) }
  })

  // Peer-protocol structures from the original's peer-protocol test
  // vectors, regenerated with the original schema so that the spec's
  // independently computed bytes are confirmed.
  const seedA = new Uint8Array(32).fill(1)
  const seedB = new Uint8Array(32).fill(3)
  const publicA = getPublicKey(seedA)
  const nodeA = calculateNodeId(publicA)
  const nodeB = calculateNodeId(getPublicKey(seedB))

  const nodeInfoValue = {
    nodeId: nodeA,
    sequence: 1_700_000_000_000n,
    url: "https://a.example",
    alias: "Node A",
    publicKey: publicA,
    entries: [
      { type: "neighbor" as const, value: { nodeId: nodeB } },
      { type: "settlementScheme" as const, value: { settlementSchemeId: "stub" } },
    ],
  }
  const nodeInfo = peerNodeInfo.serializeOrThrow(nodeInfoValue as never)
  const signature = await signAsync(nodeInfo, seedA)
  const signedNodeInfo = signedPeerNodeInfo.serializeOrThrow({
    signed: { bytes: nodeInfo },
    signature: { type: "ed25519", value: signature },
  } as never)

  const nodeListSchema = sequenceOf(nodeIdSchema)
  const nodeList = nodeListSchema.serializeOrThrow([nodeA, nodeB].sort((a, b) => a.localeCompare(b)) as never)
  const emptyNodeList = nodeListSchema.serializeOrThrow([])

  const nodeListHashRequest = peerMessage.serializeOrThrow({
    version: 0,
    sender: nodeA,
    authentication: { type: "NONE", value: undefined },
    content: { value: { type: "nodeListHashRequest", value: {} } },
  } as never)

  const linkStateUpdate = peerMessage.serializeOrThrow({
    version: 0,
    sender: nodeA,
    authentication: { type: "NONE", value: undefined },
    content: { value: { type: "linkStateUpdate", value: { bytes: signedNodeInfo } } },
  } as never)

  const sessionPrivate = new Uint8Array(32).fill(2)
  const sharedSecret = x25519.getSharedSecret(
    sessionPrivate,
    ed25519.utils.toMontgomery(getPublicKey(seedB)),
  )
  const interledgerContent = fromHex("82060000002a010c")
  const mac = calculateMessageHmac(interledgerContent, sharedSecret)
  const authenticatedInterledger = peerMessage.serializeOrThrow({
    version: 0,
    sender: nodeA,
    authentication: {
      type: "ED25519_X25519_HMAC-SHA256",
      value: {
        sessionPublicKey: x25519.getPublicKey(sessionPrivate),
        messageAuthenticationCode: mac,
      },
    },
    content: {
      value: {
        type: "interledgerPacket",
        value: { signed: { requestId: 42, packet: fromHex("0c") } },
      },
    },
  } as never)

  write("dassie-oer/tests/data/vectors.json", {
    variableUnsigned,
    objectIdentifiers,
    utf8,
    octetLengths,
    peer: {
      peerNodeInfoOid: { value: dassiePeerNodeInfoId, encoded: objectIdentifier().serializeOrThrow(dassiePeerNodeInfoId) },
      nodeInfo: { value: nodeInfoValue, nodeB, encoded: nodeInfo },
      signature,
      signedNodeInfo,
      nodeList: { encoded: nodeList, sha256: createHash("sha256").update(nodeList).digest() },
      emptyNodeList: { encoded: emptyNodeList, sha256: createHash("sha256").update(emptyNodeList).digest() },
      nodeListHashRequest,
      linkStateUpdate,
      interledgerContent,
      sharedSecret,
      mac,
      authenticatedInterledger,
    },
  })
})

test("ilp vectors", () => {
  const packets = Object.entries(ilpFixtures).map(([name, fixture]) => ({
    name,
    packet: fixture.packet,
    encoded: base64ToUint8Array(fixture.buffer),
    reencoded: serializeIlpPacket(fixture.packet as never),
  }))

  const ildcp = Object.entries(ildcpFixtures).map(([name, fixture]) => ({
    name,
    response: fixture.packet,
    encoded: base64ToUint8Array(fixture.buffer),
    reencoded: serializeIldcpResponse(fixture.packet as never),
  }))

  const f08 = [
    [0n, 0n],
    [11n, 10n],
    [18_446_744_073_709_551_615n, 1n],
  ].map(([receivedAmount, maximumAmount]) => ({
    receivedAmount,
    maximumAmount,
    encoded: serializeAmountTooLargeData({ receivedAmount: receivedAmount!, maximumAmount: maximumAmount! }),
  }))

  const timestamps = [0, 1_514_000_000_000, 1_513_992_100_549, 4_102_444_799_999].map((ms) => ({
    ms,
    text: timestampToInterledgerTime(ms),
  }))

  const ilpData = { protocolName: "ilp", contentType: BtpContentType.ApplicationOctetStream, data: base64ToUint8Array(ilpFixtures.prepare.buffer) }
  const btp = {
    message: {
      requestId: 1,
      protocolData: [
        { protocolName: "auth", contentType: BtpContentType.ApplicationOctetStream, data: new Uint8Array() },
        { protocolName: "auth_token", contentType: BtpContentType.TextPlainUtf8, data: stringToUint8Array("secret-token") },
      ],
    },
    response: { requestId: 1, protocolData: [] },
    transfer: { requestId: 0xff_ff_ff_ff, amount: 123_456_789n, protocolData: [ilpData] },
    error: {
      requestId: 7,
      code: "F00",
      name: "NotAcceptedError",
      triggeredAt: "2017-08-29T18:04:04.123Z",
      data: stringToUint8Array("nope"),
      protocolData: [],
    },
  }

  write("dassie-ilp/tests/data/vectors.json", {
    packets,
    ildcp,
    ildcpAddress: ILDCP_ADDRESS,
    ildcpCondition: ILDCP_CONDITION,
    f08,
    timestamps,
    btp: {
      message: { value: btp.message, encoded: generateBtpMessage(btp.message) },
      response: { value: btp.response, encoded: generateBtpResponse(btp.response) },
      transfer: { value: btp.transfer, encoded: generateBtpTransfer(btp.transfer) },
      error: { value: btp.error, encoded: generateBtpError(btp.error) },
    },
    packetTypes: IlpType,
  })
})

test("stream vectors", async () => {
  const packets = Object.entries(streamFixtures).map(([name, fixture]) => ({
    name,
    packet: fixture.packet,
    encoded: base64ToUint8Array(fixture.buffer),
  }))

  // The original serializer must reproduce its own fixtures; if it does not,
  // the fixture is not a vector of the original's behaviour.
  for (const fixture of packets) {
    if (fixture.name.includes("greater_than_max_uint64")) continue
    const reencoded = serializePacket(fixture.packet as never)
    if (hex(reencoded) !== hex(fixture.encoded)) {
      throw new Error(`STREAM fixture ${fixture.name} does not round-trip`)
    }
  }

  const crypto = createMockDeterministicCrypto(createCrypto())
  const secret = stringToUint8Array("foo")
  const environment = getPskEnvironment(crypto, secret)
  const encryptions = []
  for (const plaintext of ["foo bar", "", "a longer plaintext that spans more than one AES block"]) {
    encryptions.push({
      plaintext: stringToUint8Array(plaintext),
      ciphertext: await environment.encrypt(stringToUint8Array(plaintext)),
    })
  }

  write("dassie-stream/tests/data/vectors.json", {
    packets,
    crypto: {
      secret,
      encryptionKey: await environment.getEncryptionKey(),
      fulfillmentKey: await environment.getFulfillmentKey(),
      fulfillments: [
        { data: stringToUint8Array("bar"), fulfillment: await environment.getFulfillment(stringToUint8Array("bar")) },
      ],
      encryptions,
      originalCiphertext: base64ToUint8Array("HweWFgwWTh/cwwbKHqRLNeDtuakR4E8ltFSDyQ7v31ImmnQ="),
      zeroCondition: createHash("sha256").update(new Uint8Array(32)).digest(),
      hmacCheck: createHmac("sha256", secret).update("ilp_stream_encryption").digest(),
    },
  })
})
