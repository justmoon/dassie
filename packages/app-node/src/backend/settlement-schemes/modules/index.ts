import type { SettlementSchemeModule } from "../types/settlement-scheme-module"

const modules: Record<
  string,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  () => Promise<{ default: SettlementSchemeModule<any> }>
> = {
  stub: () => import("./stub"),
  "xrpl-testnet": () => import("./xrpl/xrpl-testnet"),
}

export default modules
