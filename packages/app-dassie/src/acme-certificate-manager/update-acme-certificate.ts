import { Client as AcmeClient, crypto as acmeCrypto } from "acme-client"

import { X509Certificate } from "node:crypto"

import { type Reactor, createActor } from "@dassie/lib-reactive"

import type { DassieActorContext } from "../base/types/dassie-base"
import { DatabaseConfigStore } from "../config/database-config"
import { Database } from "../database/open-database"
import { acme as logger } from "../logger/instances"
import { ACME_DIRECTORY_URL } from "./constants/acme-service"

const ACME_EXPIRY_POLLING_INTERVAL = 1000 * 60 * 60 // 1 hour
const MINIMUM_CERTIFICATE_EXPIRY = 1000 * 60 * 60 * 24 * 30 // 30 days

export const UpdateAcmeCertificateActor = (reactor: Reactor) => {
  const databaseConfigStore = reactor.use(DatabaseConfigStore)
  const database = reactor.use(Database)

  async function checkAcmeCertificateExpiry() {
    const { tlsWebCert, hostname } = databaseConfigStore.read()

    if (!tlsWebCert || !hostname) return

    const currentCertificate = new X509Certificate(tlsWebCert)

    const expiryDate = new Date(currentCertificate.validTo)
    const now = new Date()

    const expiryTime = expiryDate.getTime() - now.getTime()

    if (expiryTime > MINIMUM_CERTIFICATE_EXPIRY) return

    const accountKey = database.scalars.acmeAccountKey.get()
    const accountUrl = database.scalars.acmeAccountUrl.get()

    if (!accountKey || !accountUrl) {
      logger.warn("unable to renew certificate: no account key or URL saved")
      return
    }

    const client = new AcmeClient({
      directoryUrl: ACME_DIRECTORY_URL,
      accountKey,
      accountUrl,
    })

    logger.info("renewing tls certificate", { hostname })

    const order = await client.createOrder({
      identifiers: [{ type: "dns", value: hostname }],
    })

    const authorizations = await client.getAuthorizations(order)

    for (const authorization of authorizations) {
      const challenge = authorization.challenges.find(
        (challenge) => challenge.type === "http-01",
      )

      if (!challenge) {
        throw new Error(
          `Could not find http-01 challenge for authorization ${authorization.identifier.value}`,
        )
      }

      const keyAuthorization =
        await client.getChallengeKeyAuthorization(challenge)

      logger.info("registered challenge token", { token: challenge.token })
      database.tables.acmeTokens.upsert(
        {
          token: challenge.token,
          key_authorization: keyAuthorization,
          expires:
            authorization.expires ?
              new Date(authorization.expires).toISOString()
            : null,
        },
        ["token"],
      )

      try {
        await client.verifyChallenge(authorization, challenge)

        await client.completeChallenge(challenge)

        await client.waitForValidStatus(challenge)
      } catch (error: unknown) {
        logger.warn("failed to complete ACME challenge", { error })
        return
      } finally {
        database.tables.acmeTokens.delete({
          token: challenge.token,
        })
      }
    }

    logger.info("creating certificate signing request")

    const [key, csr] = await acmeCrypto.createCsr({
      commonName: hostname,
    })

    const finalizedOrder = await client.finalizeOrder(order, csr)

    const certificate = await client.getCertificate(finalizedOrder)

    databaseConfigStore.act.setTlsCertificates(
      certificate.toString(),
      key.toString(),
    )

    logger.info("certificate successfully renewed", { hostname })
  }

  return createActor(async (sig: DassieActorContext) => {
    const task = sig.task({
      handler: checkAcmeCertificateExpiry,
      interval: ACME_EXPIRY_POLLING_INTERVAL,
    })

    await task.execute()
    task.schedule()
  })
}
