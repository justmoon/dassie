# Implementation Notes: Configuration Wizard

## Supported deployment architectures

In the initial version of the configuration wizard, we only support nodes that are deployed as public IPs.

Eventually, we would like to also support nodes that are behind a service like Cloudflare. The idea would be that the public-facing HTTPS API is exposed through Cloudflare or another similar service. This should help protect against some attacks like denial-of-service attacks. The real IP of the node would be revealed only to the node's peers. See [Two APIs](./peer-to-peer-communication.md#two-apis).

In the case of this kind of architecture, the user may want to setup an origin certificate which the wizard should facilitate.

## Other ACME providers

Let's Encrypt is the most well-known ACME provider but by no means the only one. Some users may wish to use another provider like Google Trust Services. This is a feature we could easily add to the configuration wizard.
