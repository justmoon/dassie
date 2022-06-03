import { $, cd } from "zx"
import { resolve } from "node:path"

const SSL_CERT_PATH = resolve(__dirname, "../local/ssl")

console.log(SSL_CERT_PATH)
await $`mkdir -p ${SSL_CERT_PATH}`
cd(SSL_CERT_PATH)
await $`mkcert node1.localhost`
await $`mkcert node2.localhost`
await $`mkcert node3.localhost`
