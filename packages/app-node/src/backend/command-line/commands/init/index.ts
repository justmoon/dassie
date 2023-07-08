import {
  Client as AcmeClient,
  crypto as acmeCrypto,
  directory as acmeDirectory,
} from "acme-client"
import { command } from "cmd-ts"

import { Reactor } from "@dassie/lib-reactive"
import {
  confirm,
  createFlow,
  header,
  isCanceled,
  note,
  text,
} from "@dassie/lib-terminal-graphics"

import { connectIpcClient } from "../../../local-ipc-client"

const LETSENCRYPT_TOS_URL =
  "https://letsencrypt.org/documents/LE-SA-v1.3-September-21-2022.pdf"

export const initCommand = (reactor: Reactor) =>
  command({
    name: "init",
    description:
      "This command assists with the initial configuration and setup",
    args: {},
    async handler() {
      const ipcClient = connectIpcClient(reactor)

      const flow = createFlow()

      flow.show(header({ title: "Dassie Configuration" }))

      flow.show(
        note({
          title: "Public HTTPS Setup",
          body: "Dassie nodes must be accessible over HTTPS. This configuration tool will help you set up HTTPS using Let's Encrypt.",
        })
      )

      const domain = await flow.interact(
        text({
          title: "What is the publicly accessible domain name of this host?",
        })
      )

      if (isCanceled(domain) || !domain) {
        flow.show(
          note({
            style: "error",
            title: "Canceled. Exiting...",
          })
        )
        return
      }

      flow.show(
        note({
          title: "Let's Encrypt Service Agreement",
          body: `Please read the Terms of Service at ${LETSENCRYPT_TOS_URL}. You must agree in order to register with the ACME server.`,
        })
      )

      const tosResult = await flow.interact(confirm({ title: "Do you agree?" }))

      if (isCanceled(tosResult) || !tosResult) {
        flow.show(
          note({
            style: "error",
            title: "Let's Encrypt Service Agreement rejected",
            body: "In this preview version of Dassie, you must use Let's Encrypt to obtain a TLS certificate. Since you rejected the Let's Encrypt Service Agreement, Dassie cannot continue.",
          })
        )
        return
      }

      const email = await flow.interact(
        text({
          title: "What is your email address?",
          explanation:
            "Let's Encrypt will use this email address to contact you about your certificate.",
        })
      )

      if (isCanceled(email)) {
        flow.show(
          note({
            style: "error",
            title: "Canceled. Exiting...",
          })
        )
        return
      }

      const accountKey = await acmeCrypto.createPrivateKey()
      const client = new AcmeClient({
        directoryUrl: acmeDirectory.letsencrypt.staging,
        accountKey,
      })

      await client.createAccount({
        termsOfServiceAgreed: tosResult,
        contact: [`mailto:${email}`],
      })

      const accountUrl = client.getAccountUrl()

      await ipcClient.setNodeTlsConfiguration.mutate({
        accountUrl,
        accountKey: accountKey.toString("utf8"),
      })

      flow.show(
        note({
          style: "success",
          title: "Configuration completed!",
          body: "You may now launch Dassie!",
        })
      )
    },
  })
