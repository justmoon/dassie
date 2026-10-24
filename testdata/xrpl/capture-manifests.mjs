// Captures crates/dassie-xrpl/tests/data/manifests.json
// (spec/designs/xrpl-validation.md#13-what-pins-it), read-only: each
// publisher's manifest from its signed validator list, and the live
// manifest of each validator its network's first list names, from the
// `manifest` method, in the list's order. Each with its keys, its
// sequence and domain, and signed_bytes: MAN\0 and its fields without the
// two signatures.
//
// Usage, from the repository root:
//
//   node testdata/xrpl/capture-manifests.mjs > crates/dassie-xrpl/tests/data/manifests.json
//
// XRPL_MAINNET and XRPL_TESTNET name other servers (capture-lib.mjs).

import { assert, fetchList, hex, liveManifest, listValidators, MAINNET, manifest, open, PUBLISHERS, print, TESTNET } from "./capture-lib.mjs";

const captured = new Date().toISOString();
const publishers = [];
const validators = [];
for (const [network, url] of [["mainnet", MAINNET], ["testnet", TESTNET]]) {
  const lists = [];
  for (const site of PUBLISHERS[network]) {
    const list = listValidators(await fetchList(site));
    const bytes = Buffer.from(list.manifest, "base64");
    publishers.push({ kind: "publisher", manifest: hex(bytes), ...manifest(bytes), site: new URL(site).host });
    lists.push(list);
  }
  const connection = await open(url);
  for (const master of lists[0].validators) {
    const bytes = await liveManifest(connection, master);
    const read = manifest(bytes);
    assert(read.master === master, `the manifest of ${master}`);
    validators.push({ kind: `${network} validator`, manifest: hex(bytes), ...read });
  }
  connection.close();
}

print({
  source: `The publishers' manifests from their signed validator lists, and each UNL validator's live manifest from the manifest method; signed_bytes is MAN\\0 and the fields without the signatures. Captured ${captured} from ${
    [...PUBLISHERS.mainnet, ...PUBLISHERS.testnet].join(", ")
  }, ${MAINNET} and ${TESTNET} by testdata/xrpl/capture-manifests.mjs.`,
  manifests: [...publishers, ...validators],
});
