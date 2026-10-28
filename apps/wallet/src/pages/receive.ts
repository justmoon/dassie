/** What the Receive page decides without drawing (spec/areas/wallet.md#pages). */

/**
 * The tab a link names by a scheme, such as the Ledgers page's: its own,
 * or else its asset's, which is the hub's or the owner book's, as the node
 * names the deposit ledger of the asset. A hub scheme's ID is `hub-` before
 * the ID of the owner book it sits on (spec/decisions/scheme-names.md).
 */
export function assetTab(wanted: string, ids: readonly string[]): string {
  const same = [wanted, `hub-${wanted}`, wanted.replace(/^hub-/, "")];
  return same.find((id) => ids.includes(id)) ?? wanted;
}
