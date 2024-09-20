import { Client as AcmeClient, crypto as acmeCrypto } from "acme-client"
import { command } from "cmd-ts"

import type { Reactor } from "@dassie/lib-reactive"
import {
  type Flow,
  confirm,
  createFlow,
  header,
  isCanceled,
  note,
  text,
} from "@dassie/lib-terminal-graphics"

import { ACME_DIRECTORY_URL } from "../../../acme-certificate-manager/constants/acme-service"
import { connectIpcClient } from "../../../local-ipc-client"

const LETSENCRYPT_TOS_URL =
  "https://letsencrypt.org/documents/LE-SA-v1.3-September-21-2022.pdf"

export const InitCommand = (reactor: Reactor) =>
  command({
    name: "init",
    description:
      "This command assists with the initial configuration and setup",
    args: {},
    async handler() {
      const ipcClient = connectIpcClient(reactor)

      const flow = createFlow()

      flow.show(header({ title: "Dassie Configuration" }))

      const currentConfig = await ipcClient.config.getConfig.query()

      flow.show(
        note({
          title: "Setting realm to 'test'",
          body: "In this preview version of Dassie, only the test realm is supported. This means that you will not be able to use real money.",
        }),
      )

      await ipcClient.config.setRealm.mutate({
        realm: "test",
      })

      flow.show(
        note({
          title: "Public HTTPS Setup",
          body: "Dassie nodes must be accessible over HTTPS. This configuration tool will help you set up HTTPS using Let's Encrypt.",
        }),
      )

      const domain = await flow.interact(
        text({
          title: "What is the publicly accessible domain name of this host?",
          initialValue: currentConfig.hostname ?? "",
        }),
      )

      if (isCanceled(domain) || !domain) {
        flow.show(
          note({
            style: "error",
            title: "Canceled. Exiting...",
          }),
        )
        return
      }

      if (domain !== currentConfig.hostname) {
        await ipcClient.config.setHostname.mutate({
          hostname: domain,
        })
      }

      let forceAcme = false
      if (currentConfig.hasTls) {
        const skipAcmeResult = await flow.interact(
          confirm({
            title:
              "An existing TLS certificate was found, would you like to skip ACME/LetsEncrypt setup?",
            initialValue: true,
          }),
        )

        if (isCanceled(skipAcmeResult)) {
          flow.show(
            note({
              style: "error",
              title: "Canceled. Exiting...",
            }),
          )
          return
        }

        forceAcme = !skipAcmeResult
      }

      if (!currentConfig.hasTls || forceAcme) {
        const hasAcmeCompleted = await doAcmeFlow(flow, ipcClient, domain)

        if (!hasAcmeCompleted) {
          return
        }
      }

      if (currentConfig.hasNodeIdentity) {
        flow.show(
          note({
            style: "success",
            title: "Configuration completed!",
            body: `You may now visit your Dassie node at https://${domain}/`,
            paddingBottom: 1,
          }),
        )
      } else {
        const setupUrl = await ipcClient.config.getSetupUrl.query()

        flow.show(
          note({
            style: "success",
            title: "Configuration completed!",
            body: `You may now continue setup in your browser by visiting ${setupUrl}`,
            paddingBottom: 1,
          }),
        )
      }
    },
  })

const doAcmeFlow = async (
  flow: Flow,
  ipcClient: ReturnType<typeof connectIpcClient>,
  domain: string,
): Promise<boolean> => {
  flow.show(
    note({
      title: "Let's Encrypt Service Agreement",
      body: `Please read the Terms of Service at ${LETSENCRYPT_TOS_URL}. You must agree in order to register with the ACME server.`,
    }),
  )

  const tosResult = await flow.interact(confirm({ title: "Do you agree?" }))

  if (isCanceled(tosResult) || !tosResult) {
    flow.show(
      note({
        style: "error",
        title: "Let's Encrypt Service Agreement rejected",
        body: "In this preview version of Dassie, you must use Let's Encrypt to obtain a TLS certificate. Since you rejected the Let's Encrypt Service Agreement, Dassie cannot continue.",
        paddingBottom: 1,
      }),
    )
    return false
  }

  const email = await flow.interact(
    text({
      title: "What is your email address?",
      explanation:
        "Let's Encrypt will use this email address to contact you about your certificate.",
    }),
  )

  if (isCanceled(email)) {
    flow.show(
      note({
        style: "error",
        title: "Canceled. Exiting...",
      }),
    )
    return false
  }

  const accountKey = await acmeCrypto.createPrivateKey()
  const client = new AcmeClient({
    directoryUrl: ACME_DIRECTORY_URL,
    accountKey,
  })

  await client.createAccount({
    termsOfServiceAgreed: tosResult,
    contact: [`mailto:${email}`],
  })

  const accountUrl = client.getAccountUrl()

  await ipcClient.acme.setAcmeCredentials.mutate({
    accountUrl,
    accountKey: accountKey.toString("utf8"),
  })

  flow.show(
    note({
      title: "Ordering certificate...",
    }),
  )

  const order = await client.createOrder({
    identifiers: [{ type: "dns", value: domain }],
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

    flow.show(
      note({
        title: "Preparing challenge...",
      }),
    )

    await ipcClient.acme.registerToken.mutate({
      token: challenge.token,
      keyAuthorization,
      expires:
        authorization.expires ? new Date(authorization.expires) : undefined,
    })

    try {
      flow.show(
        note({
          title: "Verifying challenge...",
        }),
      )

      await client.verifyChallenge(authorization, challenge)

      flow.show(
        note({
          title: "Completing challenge...",
        }),
      )

      await client.completeChallenge(challenge)

      flow.show(
        note({
          title: "Waiting for confirmation from ACME provider...",
        }),
      )

      await client.waitForValidStatus(challenge)
    } finally {
      await ipcClient.acme.deregisterToken.mutate({
        token: challenge.token,
      })
    }
  }

  flow.show(
    note({
      title: "Generating certificate signing request...",
    }),
  )

  const [key, csr] = await acmeCrypto.createCsr({
    commonName: domain,
  })

  flow.show(
    note({
      title: "Finalizing order...",
    }),
  )

  const finalizedOrder = await client.finalizeOrder(order, csr)

  flow.show(
    note({
      title: "Fetching certificate...",
    }),
  )

  const certificate = await client.getCertificate(finalizedOrder)

  flow.show(
    note({
      title: "Installing certificate...",
    }),
  )

  await ipcClient.tls.setNodeTlsConfiguration.mutate({
    certificate,
    privateKey: key.toString(),
  })

  return true
}
