/**
 * The dev environment's certificates (spec 6 §6.3), made by mkcert from its
 * local CA, so that a browser trusting that CA (`mkcert -install`) trusts
 * every dev page, and the nodes trust each other through
 * `DASSIE_PEER_CA_FILE`. mkcert makes the CA on first use; `CAROOT` puts it
 * elsewhere, as the tests do.
 *
 * Certificates are kept across runs, and made again when one of the two
 * files is missing.
 *
 * @module
 */

import { exists } from "./fs.ts";

/** A certificate chain and its key, as PEM. */
export interface Certificate {
  readonly certificatePem: string;
  readonly keyPem: string;
  readonly certificateFile: string;
  readonly keyFile: string;
}

async function mkcert(args: string[]): Promise<string> {
  let output: Deno.CommandOutput;
  try {
    output = await new Deno.Command("mkcert", {
      args,
      stdin: "null",
      stdout: "piped",
      stderr: "piped",
    }).output();
  } catch (error) {
    throw new Error(`cannot run mkcert: ${error}`);
  }
  const decoder = new TextDecoder();
  if (!output.success) {
    throw new Error(
      `mkcert ${args.join(" ")} failed: ${decoder.decode(output.stderr)}`,
    );
  }
  return decoder.decode(output.stdout).trim();
}

/** mkcert's CA directory. */
export function caRoot(): Promise<string> {
  return mkcert(["-CAROOT"]);
}

/**
 * The CA's certificate, which the nodes and Deno trust, made if there is
 * none yet. The certificates in `dir` are dropped if another CA made them:
 * after `CAROOT` changed, or the CA was made anew.
 */
export async function caFile(dir: string): Promise<string> {
  const file = `${await caRoot()}/rootCA.pem`;
  if (!(await exists(file))) {
    // mkcert makes its CA with the first certificate; this one is thrown
    // away.
    const scratch = await Deno.makeTempDir({ prefix: "dassie-dev-ca-" });
    try {
      await mkcert([
        "-cert-file",
        `${scratch}/c.pem`,
        "-key-file",
        `${scratch}/k.pem`,
        "localhost",
      ]);
    } finally {
      await Deno.remove(scratch, { recursive: true });
    }
  }
  const ca = await Deno.readTextFile(file);
  const seen = `${dir}/ca.pem`;
  const known = await Deno.readTextFile(seen).catch(() => undefined);
  if (known !== ca) {
    await Deno.remove(dir, { recursive: true }).catch(() => {});
    await Deno.mkdir(dir, { recursive: true });
    await Deno.writeTextFile(seen, ca);
  }
  return file;
}

/**
 * The certificate `name` in `dir`, for `hosts`, made if it is not there.
 * The first host is the name a browser asks for.
 */
export async function certificate(
  dir: string,
  name: string,
  hosts: readonly string[],
): Promise<Certificate> {
  const certificateFile = `${dir}/${name}.pem`;
  const keyFile = `${dir}/${name}-key.pem`;
  if (!(await exists(certificateFile)) || !(await exists(keyFile))) {
    await Deno.mkdir(dir, { recursive: true });
    await Deno.remove(certificateFile).catch(() => {});
    await Deno.remove(keyFile).catch(() => {});
    await mkcert([
      "-ecdsa",
      "-cert-file",
      certificateFile,
      "-key-file",
      keyFile,
      ...hosts,
    ]);
  }
  return {
    certificatePem: await Deno.readTextFile(certificateFile),
    keyPem: await Deno.readTextFile(keyFile),
    certificateFile,
    keyFile,
  };
}

/** The dashboard's certificate: `localhost` and `dev-rpc.localhost`. */
export function dashboardCertificate(dir: string): Promise<Certificate> {
  return certificate(dir, "localhost", [
    "localhost",
    "dev-rpc.localhost",
    "127.0.0.1",
    "::1",
  ]);
}

/**
 * A node's certificate: its name, and `localhost`, at which the dev server
 * sets it up without resolving `dN.localhost` itself.
 */
export function nodeCertificate(dir: string, name: string) {
  return certificate(dir, `${name}.localhost`, [
    `${name}.localhost`,
    "localhost",
    "127.0.0.1",
  ]);
}
