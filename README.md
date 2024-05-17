# Dassie

> A multi-currency, zero-config, peer-to-peer, Interledger-enabled payment network

## Intro

[![intro](http://img.youtube.com/vi/Whp4RfW3K_U/0.jpg)](http://www.youtube.com/watch?v=Whp4RfW3K_U&t=8371 "Intro")

## Development Environment

In order to develop a peer-to-peer application, it is very useful to quickly spin up several nodes which can all talk to each other. Dassie uses HTTPS everywhere including during development, so you will need to generate certificates for your nodes.

### Prerequisites

- A Node version manager [which respects `.node-version`](https://stackoverflow.com/questions/27425852/what-uses-respects-the-node-version-file). For example, [fnm](https://github.com/Schniz/fnm).
- `*.localhost` mapped to `127.0.0.1` (on Linux this is usually the default, on MacOS use something like [dnsmasq](https://hedichaibi.com/how-to-setup-wildcard-dev-domains-with-dnsmasq-on-a-mac/)).
- [mkcert](https://github.com/FiloSottile/mkcert)
  1. Run `mkcert -install` to create the private CA and register it in your OS and browser.
  2. Add `export NODE_EXTRA_CA_CERTS="$(mkcert -CAROOT)/rootCA.pem"` to your `.bashrc`, `.zshrc`, or similar.
- [Docker](https://docs.docker.com/get-docker/) (only required if you want to build Dassie binaries)

### Setting up the development environment

Enable corepack so that the pnpm package manager can be used, then call `pnpm install` to install package dependencies.

```sh
corepack enable
pnpm install
```

### Starting the development environment

Run the development environment.

```sh
pnpm start
```

### Building Dassie binaries

You generally won't need to build Dassie images locally as this job is normally done by our CI. However, there are a few situations where you may want create custom Dassie binaries.

To initiate a build, simply run:

```sh
pnpm build
```

This will first create a "builder" Docker image and then call this image with any parameters that you pass in.

For example, you can pass in a different build target:

```sh
pnpm build release
```
