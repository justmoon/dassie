---
title: Quick Start
description: Get up and running quickly with Dassie.
---

So you want to try out Dassie. First, let me congratulate you on your bravery. Second, let me warn you that Dassie is still in the early stages of development and is not yet ready for production use. If you're still interested, read on.

## Prerequisites

You will need a server (or virtual machine) with the following:

- Linux
  - Must be a recent distribution (we did all our testing on Debian Bullseye)
  - Must use systemd
- Curl
- A publicly accessible IP address
- A domain name that points to that IP address
- TCP ports 80 and 443 open to the world

## Installation

To install Dassie, run the following command on your server:

```sh
curl --tlsv1.2 -sSf https://sh.dassie.land | sh
```

This will download and run the Dassie installer script. The script will install Dassie on your server.

If you are running as a non-root user you will need to close your SSH session and log in again at this point. This is because the installer script adds your user to the `dassie-users` group, and group membership changes only take effect the next time you log in.

## Configuration

Before we can access Dassie's snazzy web interface, we need to set you up with a TLS certificate. Fortunately, Dassie has Let's Encrypt integration built in, so it should be a quick and easy process.

Just run the initialization command:

```sh
dassie init
```

Once you have successfully set up your TLS certificate, the dassie init command will print out a URL where you can continue the setup process. If you miss the URL the first time, you can run `dassie init` again or you can find it in the Dassie logs:

```sh
journalctl -xeu dassie
```

## Updating Dassie

Dassie comes with an auto-updater which is enabled by default. So you should not have to worry about updating Dassie.

To manually trigger an update, run the following command on your server:

```sh
sudo dassie update
```

## Deinstallation

If you ever need to uninstall Dassie, you can run the following command on your server:

```sh
curl --tlsv1.2 -sSf https://sh.dassie.land | sh -s -- uninstall
```
