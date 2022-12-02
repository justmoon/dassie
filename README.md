# Dassie

> A multi-currency, zero-config, peer-to-peer, Interledger-enabled payment network

## Development Environment

In order to develop a peer-to-peer application, it is very useful to quickly spin up several nodes which can all talk to each other. Dassie uses HTTPS everywhere including during development, so you will need to generate certificates for your nodes.

### Prerequisites

- Node.js 18
- [mkcert](https://github.com/FiloSottile/mkcert)
  1. Run `mkcert -install` to create the private CA and register it in your OS and browser.
  2. Add `export NODE_EXTRA_CA_CERTS="$(mkcert -CAROOT)/rootCA.pem"` to your `.bashrc`, `.zshrc`, or similar.
- OpenSSL with ed25519 support. On MacOS Sierra and higher this requires [further steps](https://github.com/justmoon/dassie/issues/3#issuecomment-1312636093).

### Setting up the development environment

Install package dependencies.

```sh
pnpm i
```

### Starting the development environment

Run the development environment.

```sh
pnpm start
```
