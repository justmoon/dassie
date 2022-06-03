# Xen/ILP

> A multi-currency, zero-config, peer-to-peer, Interledger-enabled payment network

## Development Environment

In order to develop a peer-to-peer application, it is very useful to quickly spin up several nodes which can all talk to each other. Xen/ILP uses HTTPS everywhere including during development, so you will need to generate certificates for your nodes.

### Generate certificates

1. Install [mkcert](https://github.com/FiloSottile/mkcert) which is a tool for managing your own private CA.
2. Run `mkcert -install` to create the private CA and register it in your OS and browser.
3. Add `export NODE_EXTRA_CA_CERTS="$(mkcert -CAROOT)/rootCA.pem"` to your `.bashrc`, `.zshrc`, or similar.
3. Run `pnpm run dev:certs` to generate the certificates.