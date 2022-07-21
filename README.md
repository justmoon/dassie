# Xen/ILP

> A multi-currency, zero-config, peer-to-peer, Interledger-enabled payment network

## Development Environment

In order to develop a peer-to-peer application, it is very useful to quickly spin up several nodes which can all talk to each other. Xen/ILP uses HTTPS everywhere including during development, so you will need to generate certificates for your nodes.

### Prerequisites

- Node.js 18
- [mkcert](https://github.com/FiloSottile/mkcert)
  1. Run `mkcert -install` to create the private CA and register it in your OS and browser.
  2. Add `export NODE_EXTRA_CA_CERTS="$(mkcert -CAROOT)/rootCA.pem"` to your `.bashrc`, `.zshrc`, or similar.

### Starting the development environment

Run the development environment with:

```sh
DEBUG=xen:* pnpm dev
```
