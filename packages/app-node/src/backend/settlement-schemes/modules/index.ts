import type { SettlementSchemeModule } from "../types/settlement-scheme-module"
import stub from "./stub"
import xrplTestnet from "./xrpl/xrpl-testnet"

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const modules: Record<string, SettlementSchemeModule<any>> = {
  stub,
  "xrpl-testnet": xrplTestnet,
}

export default modules
