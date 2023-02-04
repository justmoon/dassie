import { bytesToHex } from "@noble/hashes/utils"
import * as bip39 from "@scure/bip39"
import { wordlist } from "@scure/bip39/wordlists/english"
import { ArrowLeft } from "phosphor-react"
import { useCallback, useState } from "react"

import { useSig } from "@dassie/lib-reactive-trpc/client"

import Dialog from "../../components/dialog/dialog"
import { walletStore } from "../../stores/wallet"
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
  const sig = useSig()

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

  const onMnemonic = useCallback(
    (mnemonic: string) => {
      setSubpage({ subpage: "opening", mnemonic })

      void bip39
        .mnemonicToSeed(mnemonic)
        .then((binarySeed) =>
          sig.use(walletStore).setSeed(bytesToHex(binarySeed))
        )
    },
    [setSubpage, sig]
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
          />
        )
      }
      case "verify": {
        return (
          <SubpageVerify mnemonic={subpage.mnemonic} onConfirm={onMnemonic} />
        )
      }
      case "recover": {
        return <SubpageRecover onConfirm={onMnemonic} />
      }
      case "opening": {
        return null
      }
    }
  })()

  return (
    <div className="flex h-full items-center justify-center">
      <Dialog.Root>
        <Dialog.Titlebar>
          {subpage.subpage === "intro" ? null : (
            <Dialog.TitleActionButton onClick={onBack}>
              <ArrowLeft />
              <span className="sr-only">Go back</span>
            </Dialog.TitleActionButton>
          )}
          <h1 className="flex-grow flex-shrink-0 basis-auto font-bold text-lg md:text-xl">
            {SUBPAGE_TITLES[subpage.subpage]}
          </h1>
        </Dialog.Titlebar>
        {subpageElement}
      </Dialog.Root>
    </div>
  )
}
