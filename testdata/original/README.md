# Golden vectors from the original

`generate-vectors.test.ts` produces the golden vectors that the core crates are tested against, using the original Dassie implementation at commit `e962ab78`. It writes:

| File | Contents |
|---|---|
| `crates/dassie-identity/tests/data/chain.json` | A spread of the dev vanity table (entropy → mnemonic → seed → node ID), extended with node key, public key, login secret, dev session token, settlement entropy and the XRPL wallets |
| `crates/dassie-identity/tests/data/keys.json` | Bootstrap node IDs, the seed `01`×32 and `03`×32 keys with signatures and X25519 conversions, and the session shared secret of the original's peer-protocol test vectors |
| `crates/dassie-oer/tests/data/vectors.json` | lib-oer's sample tables (variable unsigned integers, object identifiers, UTF-8 strings, length determinants) and the peer-protocol structures of the original's peer-protocol test vectors |
| `crates/dassie-ilp/tests/data/vectors.json` | ILP and ILDCP fixtures, F08 data, Interledger timestamps and BTP messages |
| `crates/dassie-stream/tests/data/vectors.json` | All STREAM packet fixtures and the STREAM crypto vectors |

Every file records `"source": "e962ab78"`. The vectors describe the original's behaviour. Where the rewrite deliberately differs, the Rust tests carry their own hand-written vector, named after what they check.

## Regenerating

The generator imports the original's modules, so it runs inside a copy of the original tree. It is a Vitest file because Vitest is the only runner there that resolves the original's extensionless TypeScript imports.

```sh
ORIG=$(mktemp -d)
git archive e962ab78 | tar -x -C "$ORIG"
# The lockfile at e962ab78 is out of date with its package.json files.
(cd "$ORIG" && pnpm install --no-frozen-lockfile --ignore-scripts --config.engine-strict=false)
cp testdata/original/generate-vectors.test.ts "$ORIG/packages/app-dassie/src/"
(cd "$ORIG" && VECTORS_OUT="$PWD/crates" npx vitest run packages/app-dassie/src/generate-vectors.test.ts)
```

Run the last command from the repository root, so that `$PWD/crates` is this repository's `crates/` directory. The output is deterministic, so regenerating must not change any committed vector. If it does, find out why before committing.
