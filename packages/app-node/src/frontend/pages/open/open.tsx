import { bytesToHex } from "@noble/hashes/utils"
import * as bip39 from "@scure/bip39"
import { wordlist } from "@scure/bip39/wordlists/english"
import { useCallback, useState } from "react"

import { Card, CardHeader, CardTitle } from "../../components/ui/card"
import { SEED_PATH_NODE } from "../../constants/seed-paths"
import { walletStore } from "../../stores/wallet"
import { getPrivateSeedAtPath } from "../../utils/signer"
import { trpc } from "../../utils/trpc"
import { SubpageGenerate } from "./subpage-generate/subpage-generate"
import { SubpageIntro } from "./subpage-intro/subpage-intro"
import { SubpageRecover } from "./subpage-recover/subpage-recover"
import { SubpageVerify } from "./subpage-verify/subpage-verify"

interface SubpageIntroState {
  subpage: "intro"
}

interface SubpageGenerateState {
  subpage: "generate"
  mnemonic: string
}

interface SubpageVerifyState {
  subpage: "verify"
  mnemonic: string
}

interface SubpageRecoverState {
  subpage: "recover"
  mnemonic: string
}

interface SubpageOpeningState {
  subpage: "opening"
  mnemonic: string
}

type SubpageState =
  | SubpageIntroState
  | SubpageGenerateState
  | SubpageVerifyState
  | SubpageRecoverState
  | SubpageOpeningState

const SUBPAGE_TITLES: Record<SubpageState["subpage"], string> = {
  intro: "Dassie Wallet",
  generate: "Generate Wallet",
  verify: "Verify Passphrase",
  recover: "Recover Wallet",
  opening: "Opening Wallet",
} as const

export const Open = () => {
  const [subpage, setSubpage] = useState<SubpageState>({ subpage: "intro" })
  const setNodeIdentity = trpc.config.setNodeIdentity.useMutation()

  const onBack = useCallback(() => {
    switch (subpage.subpage) {
      case "generate": {
        setSubpage({ subpage: "intro" })
        break
      }
      case "verify": {
        setSubpage({ subpage: "generate", mnemonic: subpage.mnemonic })
        break
      }
      case "recover": {
        setSubpage({ subpage: "intro" })
        break
      }
    }
  }, [subpage])

  const onGotoGenerate = useCallback(() => {
    setSubpage({
      subpage: "generate",
      mnemonic: bip39.generateMnemonic(wordlist),
    })
  }, [setSubpage])

  const onGenerateConfirm = useCallback((mnemonic: string) => {
    setSubpage({ subpage: "verify", mnemonic })
  }, [])

  const onGotoRecover = useCallback(() => {
    setSubpage({ subpage: "recover", mnemonic: "" })
  }, [setSubpage])

  const setNodeIdentityMutate = setNodeIdentity.mutate
  const onMnemonic = useCallback(
    (mnemonic: string) => {
      setSubpage({ subpage: "opening", mnemonic })

      void bip39.mnemonicToSeed(mnemonic).then((binarySeed) => {
        setNodeIdentityMutate({
          rawDassieKeyHex: bytesToHex(
            getPrivateSeedAtPath(binarySeed, SEED_PATH_NODE)
          ),
        })
        walletStore.setSeed(bytesToHex(binarySeed))
      })
    },
    [setSubpage, setNodeIdentityMutate]
  )

  const subpageElement = (() => {
    switch (subpage.subpage) {
      case "intro": {
        return (
          <SubpageIntro
            onGenerateClick={onGotoGenerate}
            onRecoverClick={onGotoRecover}
          />
        )
      }
      case "generate": {
        return (
          <SubpageGenerate
            mnemonic={subpage.mnemonic}
            onConfirm={onGenerateConfirm}
            onBack={onBack}
          />
        )
      }
      case "verify": {
        return (
          <SubpageVerify
            mnemonic={subpage.mnemonic}
            onConfirm={onMnemonic}
            onBack={onBack}
          />
        )
      }
      case "recover": {
        return <SubpageRecover onConfirm={onMnemonic} onBack={onBack} />
      }
      case "opening": {
        return null
      }
    }
  })()

  return (
    <div className="flex h-full items-center justify-center">
      <Card>
        <CardHeader>
          <CardTitle>{SUBPAGE_TITLES[subpage.subpage]}</CardTitle>
        </CardHeader>
        {subpageElement}
      </Card>
    </div>
  )
}
