/**
 * Whether a payment pointer names the node at the default front door,
 * `<label>.dassiepay.net`, as a node without a hostname is named
 * (spec/areas/payments.md#the-nodes-names). Front doors do not pass
 * payments on yet, so nothing reaches the node there.
 */
export function atFrontDoor(pointer: string): boolean {
  const host = pointer.replace(/^\$/, "").split("/")[0].split(":")[0];
  return host.toLowerCase().endsWith(".dassiepay.net");
}
