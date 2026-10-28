// The keeper's IO: the XRP Ledger testnet through xrpl.js, its faucet, and
// the exchange's ticker. Every request has a timeout and every wait a
// bound; what comes back is parsed by the pure modules.

import {
  Client,
  RippledError,
  type SubmittableTransaction,
  type Wallet,
} from "xrpl";
import { type Addresses, type Submission } from "./transactions.ts";
import type { State } from "./plan.ts";
import {
  type Account,
  parseAccount,
  parseLine,
  parseLpTokens,
  parsePool,
} from "./state.ts";
import { DSUSD_CURRENCY, type Result } from "./units.ts";

/** The testnet's network ID: the keeper acts on no other ledger. */
export const TESTNET_NETWORK_ID = 1;
/** Timeouts: connecting, each request, the faucet and the ticker. A
 * submitted transaction is waited for until its last ledger has passed,
 * which xrpl.js sets a bounded number of ledgers ahead. */
const CONNECT_TIMEOUT_MS = 10_000;
const REQUEST_TIMEOUT_MS = 20_000;
const HTTP_TIMEOUT_MS = 20_000;
/** The largest HTTP answer read, in bytes. */
const HTTP_BODY_BYTES_MAX = 64 * 1024;
/** How often, and how long apart, the keeper looks for the faucet's XRP
 * to arrive. */
const FAUCET_POLLS_MAX = 30;
const FAUCET_POLL_MS = 1_000;
/** Lines read per `account_lines` request: with a peer named, the keeper
 * has one or two. */
const LINES_MAX = 20;

/** What a following keeper hears from its connection: each transaction
 * stream message, and the connection lost. */
export type Listeners = {
  transaction: (message: unknown) => void;
  lost: () => void;
};

export class Ledger {
  #client: Client;
  /** The pool's account as the last read found it, and the account whose
   * transactions the connection streams. */
  #poolAccount: string | null = null;
  #followed: string | null = null;
  #closing = false;

  private constructor(client: Client) {
    this.#client = client;
  }

  /** Connects to `server`, and checks that it serves the testnet. With
   * `listeners`, it tells them what it streams, and once that the
   * connection is lost: xrpl.js connects again by itself, but without the
   * subscription, so a following keeper opens a new one. */
  static async open(server: string, listeners?: Listeners): Promise<Ledger> {
    const client = new Client(server, {
      connectionTimeout: CONNECT_TIMEOUT_MS,
      timeout: REQUEST_TIMEOUT_MS,
    });
    await client.connect();
    const ledger = new Ledger(client);
    if (listeners !== undefined) {
      client.on("transaction", (message) => listeners.transaction(message));
      client.once("disconnected", () => {
        if (!ledger.#closing) listeners.lost();
      });
    }
    try {
      const info = await client.request({ command: "server_info" });
      const network = (info.result.info as { network_id?: unknown })
        .network_id;
      if (network !== TESTNET_NETWORK_ID) {
        throw new Error(`${server} serves network ${network}, not the testnet`);
      }
    } catch (error) {
      await ledger.close();
      throw error;
    }
    return ledger;
  }

  async close(): Promise<void> {
    this.#closing = true;
    await this.#client.disconnect();
  }

  /** The pool's account, as the last read found it. */
  get poolAccount(): string | null {
    return this.#poolAccount;
  }

  /** Has the server stream the validated transactions that touch
   * `account`; one it streams already is not asked again. Whether it
   * subscribed now. A testnet reset makes the pool anew, at another
   * account, which a run then follows. */
  async follow(account: string): Promise<boolean> {
    if (this.#followed === account) return false;
    await this.#client.request({ command: "subscribe", accounts: [account] });
    this.#followed = account;
    return true;
  }

  /** The state the plan works from, as of the newest validated ledger. */
  async read(addresses: Addresses): Promise<State> {
    const issuer = await this.#account(addresses.issuer);
    const keeper = await this.#account(addresses.keeper);
    if (issuer === null || keeper === null) {
      return { issuer, keeper, line: null, pool: null };
    }
    const line = expect(
      parseLine(await this.#lines(addresses.keeper, addresses.issuer)),
    );
    const amm = await this.#request({
      command: "amm_info",
      asset: { currency: "XRP" },
      asset2: { currency: DSUSD_CURRENCY, issuer: addresses.issuer },
      ledger_index: "validated",
    });
    if (amm === null) return { issuer, keeper, line, pool: null };
    const { pool, account, lpCurrency } = expect(
      parsePool(amm, addresses.issuer),
    );
    this.#poolAccount = account;
    const lpKeeper = expect(
      parseLpTokens(await this.#lines(addresses.keeper, account), lpCurrency),
    );
    return { issuer, keeper, line, pool: { ...pool, lpKeeper } };
  }

  /** Signs `submission.tx` with `wallet`, submits it and waits for it to
   * be validated; anything but tesSUCCESS is an error. Returns its hash. */
  async submit(submission: Submission, wallet: Wallet): Promise<string> {
    const response = await this.#client.submitAndWait(
      submission.tx as unknown as SubmittableTransaction,
      { wallet, autofill: true },
    );
    const meta = response.result.meta;
    const code = typeof meta === "object" && meta !== null
      ? meta.TransactionResult
      : "no metadata";
    if (code !== "tesSUCCESS") {
      throw new Error(`${submission.tx.TransactionType}: ${code}`);
    }
    return response.result.hash;
  }

  /** Asks `faucet` for XRP for `address`, and waits until it arrives. */
  async fund(faucet: string, address: string): Promise<void> {
    const before = (await this.#account(address))?.drops ?? 0n;
    const answer = await post(faucet, { destination: address });
    if (!answer.ok) throw new Error(`faucet: ${answer.error}`);
    for (let poll = 0; poll < FAUCET_POLLS_MAX; poll += 1) {
      await new Promise((resolve) => setTimeout(resolve, FAUCET_POLL_MS));
      const after = await this.#account(address);
      if (after !== null && after.drops > before) return;
    }
    throw new Error(`faucet: no XRP reached ${address}`);
  }

  async #account(address: string): Promise<Account | null> {
    const result = await this.#request({
      command: "account_info",
      account: address,
      ledger_index: "validated",
    });
    return result === null ? null : expect(parseAccount(result));
  }

  async #lines(account: string, peer: string): Promise<unknown> {
    const result = await this.#request({
      command: "account_lines",
      account,
      peer,
      limit: LINES_MAX,
      ledger_index: "validated",
    });
    if (result === null) throw new Error(`account_lines: ${account} not found`);
    return result;
  }

  /** A request's result, or null when the server says the account or
   * pool asked for does not exist. */
  async #request(request: Record<string, unknown>): Promise<unknown> {
    try {
      // xrpl.js types each request; these are checked by the parsers.
      const response = await this.#client.request(
        request as unknown as Parameters<Client["request"]>[0],
      );
      return (response as { result: unknown }).result;
    } catch (error) {
      if (
        error instanceof RippledError &&
        (error.data as { error?: unknown } | undefined)?.error === "actNotFound"
      ) {
        return null;
      }
      throw error;
    }
  }
}

/** A parsed answer, or the run stops on it. */
function expect<T>(result: Result<T>): T {
  if (!result.ok) throw new Error(result.error);
  return result.value;
}

/** GETs `url` as JSON, bounded in time and size. */
export async function getJson(url: string): Promise<Result<unknown>> {
  return await http(url, { method: "GET" });
}

async function post(url: string, body: unknown): Promise<Result<unknown>> {
  return await http(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

async function http(url: string, init: RequestInit): Promise<Result<unknown>> {
  let response: Response;
  try {
    response = await fetch(url, {
      ...init,
      signal: AbortSignal.timeout(HTTP_TIMEOUT_MS),
    });
  } catch (error) {
    return { ok: false, error: `${url}: ${String(error)}` };
  }
  const text = await readBounded(response);
  if (!text.ok) return text;
  if (!response.ok) {
    return {
      ok: false,
      error: `${url}: ${response.status} ${text.value.slice(0, 200)}`,
    };
  }
  try {
    return { ok: true, value: JSON.parse(text.value) as unknown };
  } catch (_error) {
    return { ok: false, error: `${url}: not JSON` };
  }
}

async function readBounded(response: Response): Promise<Result<string>> {
  if (response.body === null) return { ok: true, value: "" };
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > HTTP_BODY_BYTES_MAX) {
        await reader.cancel();
        return { ok: false, error: `${response.url}: answer too large` };
      }
      chunks.push(value);
    }
  } catch (error) {
    return { ok: false, error: `${response.url}: ${String(error)}` };
  }
  return { ok: true, value: Buffer.concat(chunks).toString("utf8") };
}
