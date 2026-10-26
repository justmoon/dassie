/**
 * The dev environment's exchange rates: `rates.json` in Coinbase's
 * format, served over HTTPS under the scenario's CA, which every node uses
 * as its only rate source. Fixed rates make dev payments across currencies
 * easy to follow, and tests change them.
 *
 * @module
 */

import type { Certificate } from "./certs.ts";
import { serve } from "./serve.ts";

/**
 * Where the rates are served, with the port offset added: beside the fake
 * ledger's ports.
 */
export const RATES_PORT = 10210;

/** Units of each currency one dollar buys. */
export type RateTable = Readonly<Record<string, string>>;

/** The dev rates: a dollar buys two XRP. */
export const DEV_RATES: RateTable = {
  USD: "1",
  XRP: "2",
  EUR: "0.92",
};

/** `rates.json` for `rates`. */
export function ratesJson(rates: RateTable): string {
  return JSON.stringify({ data: { currency: "USD", rates } });
}

/** A running rate source. */
export interface RateServer {
  /** Its `rates.json` URL. */
  readonly url: string;
  /** Answers these rates from now on; none answers 503. */
  set(rates: RateTable | undefined): void;
  stop(): Promise<void>;
}

/** Serves `rates` on {@linkcode RATES_PORT} plus `portOffset`. */
export async function startRates(
  certificate: Certificate,
  portOffset = 0,
  rates: RateTable = DEV_RATES,
): Promise<RateServer> {
  let current: RateTable | undefined = rates;
  const port = RATES_PORT + portOffset;
  const server = await serve({
    hostname: "127.0.0.1",
    port,
    cert: certificate.certificatePem,
    key: certificate.keyPem,
  }, (request) => {
    if (new URL(request.url).pathname !== "/rates.json") {
      return new Response("not found", { status: 404 });
    }
    if (!current) return new Response("unavailable", { status: 503 });
    return new Response(ratesJson(current), {
      headers: { "content-type": "application/json" },
    });
  });
  return {
    url: `https://localhost:${port}/rates.json`,
    set(rates) {
      current = rates;
    },
    async stop() {
      await server.shutdown();
    },
  };
}
