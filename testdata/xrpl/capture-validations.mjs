// Captures crates/dassie-xrpl/tests/data/validations.json
// (spec/designs/xrpl-validation.md#13-what-pins-it), read-only: on each
// network, every validation the `validations` stream carries for one
// ledger, each as the stream gave it (data is the serialized
// STValidation), once the whole UNL has signed it; the ledger's header
// from the `ledger` method; the UNL from the first publisher's list in
// its order, each validator with its live signing key from the `manifest`
// method; and rippled's quorum from `server_info`.
//
// Usage, from the repository root:
//
//   node testdata/xrpl/capture-validations.mjs > crates/dassie-xrpl/tests/data/validations.json
//
// XRPL_MAINNET and XRPL_TESTNET name other servers (capture-lib.mjs); each
// must stream validations, as a rippled server does.

import { assert, fetchList, headerFields, ledgerHeader, liveManifest, listValidators, MAINNET, manifest, open, PUBLISHERS, print, TESTNET } from "./capture-lib.mjs";

// How long to wait for the whole UNL to sign one ledger, and for the
// validations of others after it has.
const WHOLE_UNL_MS = 30_000;
const STRAGGLERS_MS = 5_000;
const ATTEMPTS = 5;
const MAX_VALIDATIONS = 10_000;

const KEPT = ["data", "validation_public_key", "flags", "signing_time", "master_key"];

// The validations of a ledger the stream carries from its start: the one
// two after the first a UNL validator signs, which the whole UNL must sign
// within the time allowed.
function collect(connection, unl, attempt) {
  const signing = new Set(unl.map((validator) => validator.signing_b58));
  return new Promise((resolve, reject) => {
    let target = null;
    const seen = [];
    const signed = new Set();
    let finish = null;
    const timeout = setTimeout(
      () => reject(new Error(`attempt ${attempt}: ${signed.size} of ${unl.length} validators signed ledger ${target}`)),
      WHOLE_UNL_MS,
    );
    connection.onStream = (message) => {
      if (message.type !== "validationReceived" || !signing.has(message.validation_public_key)) {
        if (target !== null && message.type === "validationReceived" && Number(message.ledger_index) === target) {
          seen.push(message);
        }
        return;
      }
      // A ledger the UNL has not begun to sign when the stream starts.
      target ??= Number(message.ledger_index) + 2;
      if (Number(message.ledger_index) !== target) return;
      assert(seen.length < MAX_VALIDATIONS, "validations within bounds");
      seen.push(message);
      signed.add(message.validation_public_key);
      if (signed.size === unl.length && finish === null) {
        clearTimeout(timeout);
        finish = setTimeout(() => resolve({ target, seen }), STRAGGLERS_MS);
      }
    };
  });
}

async function capture(network, url) {
  const list = listValidators(await fetchList(PUBLISHERS[network][0]));
  const connection = await open(url);
  const unl = [];
  for (const master of list.validators) {
    const { master_b58, signing, signing_b58 } = manifest(await liveManifest(connection, master));
    unl.push({ master, master_b58, signing, signing_b58 });
  }
  const quorum = (await connection.call("server_info")).info.validation_quorum;
  assert(Number.isInteger(quorum) && quorum > 0 && quorum <= unl.length, `a quorum: ${quorum}`);
  await connection.call("subscribe", { streams: ["validations"] });
  let collected = null;
  for (let attempt = 1; collected === null; attempt++) {
    try {
      collected = await collect(connection, unl, attempt);
    } catch (error) {
      if (attempt >= ATTEMPTS) throw error;
      process.stderr.write(`${network}: ${error.message}; again\n`);
    }
  }
  connection.onStream = () => {};
  await connection.call("unsubscribe", { streams: ["validations"] });
  const header = await ledgerHeader(connection, collected.target);
  connection.close();
  // Validations of another ledger at the same index are not this one's.
  const validations = collected.seen
    .filter((message) => message.ledger_hash === header.hash)
    .map((message) => Object.fromEntries(KEPT.filter((key) => key in message).map((key) => [key, message[key]])));
  const unlSigned = new Set(validations.map((entry) => entry.validation_public_key).filter((key) => unl.some((v) => v.signing_b58 === key)));
  assert(unlSigned.size === unl.length, `the whole UNL signed ledger ${header.hash}`);
  return {
    network,
    ledger_index: header.index,
    ledger_hash: header.hash,
    quorum,
    header: { ...headerFields(header.json), ledger_index: header.json.ledger_index },
    unl,
    validations,
  };
}

const captured = new Date().toISOString();
const ledgers = [await capture("mainnet", MAINNET), await capture("testnet", TESTNET)];
print({
  source: `Every validation the validations stream carried for one ledger on each network, each as the stream gave it: data is the serialized STValidation. unl is the network's UNL from its publisher's list, each validator with its live signing key from the manifest method; quorum is rippled's. Captured ${captured} from ${MAINNET} and ${TESTNET} (rippled API v1) by testdata/xrpl/capture-validations.mjs.`,
  ledgers,
});
