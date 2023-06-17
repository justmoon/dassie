# Implementation Notes: Install Command

## Using `curl | sh`

The `curl | sh` is common but there are criticisms. The Sandstorm blog provides a good breakdown of the risks:

https://sandstorm.io/news/2015-09-24-is-curl-bash-insecure-pgp-verified-install

Summarizing the key points:

- Most forms of software distribution still involve granting excessive privileges and access to the distributor. `curl | sh` is not unique in that respect.
- The primary layer of security that it is missing is a cryptographic signature from the authors.
- It is darn convenient.

When using this pattern however, there are a few pitfalls we can avoid.

## Don't allow redirects

There is no need to allow redirects - curl doesn't follow them unless provided with the `-L` flag so we deliberately don't provide that flag.

## Prevent insecure connections

When curl is loading the script, there is a risk that the connection could be insecure and malicious actor might impersonate the server and send a malicious script instead of the intended installation script.

For example, at the time of writing, rustup uses the following commandline:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

The `--tlsv1.2` ensures that the client will only accept versions of TLS that are currently considered secure. The `--proto '=https'` options means that only HTTPS can be used. This part is unnecessary to specify since curl will not attempt HTTP if the URL starts with `https://` and without the `-L` option it will not follow redirects.

So we could simplify the above to:

```sh
curl --tlsv1.2 -sSf https://sh.dassie.land | sh
```

## Pitfalls of pipes

Using pipes is [detectable on the server-side](https://lukespademan.com/blog/the-dangers-of-curlbash/) which may allow a malicious actor to evade detection for longer by only delivering a malicious version of the script if they are sure that is directly piped into a shell.

One alternative is to use `sh -c` instead which causes curl to download the entire script before the shell is invoked. However, there are limits to the size of arguments which vary between platforms so there are potential compatibility concerns.

## Avoid partial downloads

If a script was partially downloaded, it could cause problems when executed. For example, an innocuous command like `rm -rf /tmp/download-123/` could be cut off and turn into `rm -rf /`.

Unfortunately, even with `--fail`/`-f`, curl will still output the partially downloaded file, even if the connection was interrupted and the downloaded content is smaller than the HTTP `Content-Length` header would indicate.

The solution used by Sandstorm is to wrap the entire script in a function which is called at the end. This protects against truncation but does not validate the integrity of the script. (It's pretty rare for an HTTP download to get corrupted thanks to TCP checksums, so this may be good enough.)

Another option would be to have the script validate itself. (This only works if the script is downloaded into a file.) For example:

```sh
#!/bin/bash

SCRIPT_CHECKSUM="a24b0f1559e51e0a889191fad61856d94a3b2e49883d084cda123d5d0cb9d7b2"

SELF_CHECKSUM=$(tail -n +5 $0 | sha256sum | cut -d' ' -f1)

if [ "$SELF_CHECKSUM" != "$SCRIPT_CHECKSUM" ]; then
  echo "Checksum validation failed! Exiting."
  exit 1
fi

# The rest of the script starts here
```

## Making the script more auditable

It would be nice if the install script was easy to read with very little control flow. One possible way of achieving this would be to move some of the logic to the server. Instead of a script which has to check a bunch of different things on the local system, we submit some of the information about the local system to the server which will then send a custom script. This could be done via HTTP headers, for example:

```sh
curl -H "Uname: $(uname -sm)" ...
```

The main downside of this approach is that the resulting script would not be static and therefore be harder to audit in other ways. For instance, rather than having one known checksum, there would be many variations.

## Validating best practices

We can use [Shellcheck](https://www.shellcheck.net/) to verify the script follows certain best practices for shell scripts. For example, that's what Docker does: https://github.com/docker/docker-install/blob/f529a2b4c278c5f36269bf2cbd5bac0d0adec8b8/Makefile

## Testing interactions

For testing any interactive parts of the install script, we can use the CLI testing library: https://github.com/gmrchk/cli-testing-library

## Verifying signatures

In order to allow users to verify releases signed by the developers, they will need known good keys of the developers and then verify the signature against these.

To reduce the chance that the key list is compromised, we could host it in multiple places and verify that each of those copy matches exactly.

Another minor issue is that with a simple `gpg --verify`, any locally installed, trusted key would be valid for signing the binary. It makes sense to restrict the keys to only the expected Dassie developer keys by using a temporary keyring.

Here is a script incorporating both of these considerations:

```sh
#!/bin/bash
set -e

VERSION="0.0.1"
FILENAME="dassie-${VERSION}-linux-x64.tar.xz"
KEYRING="./dassie-keyring.gpg"

# download the keyfiles
curl -s https://keybase.io/dassie/pgp_keys.asc -o keybase.asc
curl -s https://dassie.land/pgp_keys.asc -o dassie.asc

# compare the keyfiles
if ! cmp -s "keybase.asc" "dassie.asc"; then
   echo "FAILURE: The keys are different."
   exit 1
fi

# import the keys into a temporary keyring
gpg --no-default-keyring --keyring ${KEYRING} --import keybase.asc

# download SHASUMS256.txt and SHASUMS256.txt.sig
curl -s "https://get.dassie.land/${VERSION}/SHASUMS256.txt" -o SHASUMS256.txt
curl -s "https://get.dassie.land/${VERSION}/SHASUMS256.txt.sig" -o SHASUMS256.txt.sig

# verify the signature
if ! gpg --no-default-keyring --keyring ${KEYRING} --verify SHASUMS256.txt.sig SHASUMS256.txt; then
    echo "FAILURE: The signature is invalid."
    exit 1
fi

# download the file to be verified
curl -s "https://get.dassie.land/${VERSION}/${FILENAME}" -o "${FILENAME}"

# verify the checksum
if ! grep "${FILENAME}" SHASUMS256.txt | sha256sum -c -; then
    echo "FAILURE: Checksum did not match."
    exit 1
fi

# cleanup
rm -f keybase.asc dassie.asc ${KEYRING} SHASUMS256.txt SHASUMS256.txt.sig

echo "VERIFIED"
```

## Detecting color support

There are a couple of ways that you can detect whether a terminal supports colors. But not really any good one.

Terminals (are supposed to) set the TERM environment variable to a unique string so one could use a database (such as
terminfo) to figure out what the capabilities of that terminal are.

Of course, for something like an install script, we don't want to ship a huge database of terminal names.

Most modern terminals support 24-bit colors and those that do set the `COLORTERM` environment variable. If we see that,
colors are definitely supported. But there are plenty of terminals which support at least 16 colors but don't support
24-bit color and therefore don't set the `COLORTERM` variable.

Since practically all terminals support color these days, many command-line utilities assume color support if they are
running in a terminal. There is a [convention](https://no-color.org/) to disable colors when the "NO_COLOR" environment
variable is present and non-empty.

Another strong signal that colors should not be used is when the TERM variable is set to "dumb".

Finally, we have CI environments, which aren't terminals, but many of them do support colors. So we could either list
CI environments that are known to support colors (again the database approach) or assume they do and just react to the
CI environment variable.

Overall, a simple heuristic would be:

- Check for the FORCE_COLOR environment variable. It's value overrides everything.
- Check for the NO_COLOR environment varible. If set, disable color.
- If stdout is a terminal
  - If TERM is set to "dumb", disable colors
  - Otherwise, enable colors
- Otherwise, disable colors

## Examples

- https://rustup.rs/
- https://github.com/pnpm/get.pnpm.io
- https://ohmyz.sh/#install
- https://github.com/docker/docker-install
- https://rclone.org/install/

## References

- https://0x46.net/thoughts/2019/04/27/piping-curl-to-shell/
- https://lukespademan.com/blog/the-dangers-of-curlbash/
- https://unix.stackexchange.com/questions/339237/whats-the-difference-between-curl-sh-and-sh-c-curl
- https://blog.pan-net.cloud/posts/curl-security-anti-patterns/
