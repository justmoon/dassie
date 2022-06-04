import { $, cd } from "zx"

const CERT_PATH = new URL("../../../local/ssl", import.meta.url).pathname

console.log(CERT_PATH)
await $`mkdir -p ${CERT_PATH}`
cd(CERT_PATH)
await $`mkcert node1.localhost`
await $`mkcert node2.localhost`
await $`mkcert node3.localhost`
