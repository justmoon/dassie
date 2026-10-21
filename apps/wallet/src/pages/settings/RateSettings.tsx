/**
 * Exchange rates (spec/decisions/exchange-rates.md): the currency totals are
 * shown in, and where the node fetches rates. A rate is the median of the
 * sources that give one, once more than half of them do.
 */

import type { JSX } from "@solidjs/web";
import type { RateSource } from "@dassie/rpc/proto";
import { createMemo, createSignal, createUniqueId, For, Show } from "solid-js";
import { useNodeData } from "@dassie/ui/node-data";
import { describe, useClient } from "@dassie/ui/rpc";
import { useToast } from "@dassie/ui/toast";
import { Card, ErrorText, Field } from "@dassie/ui/ui";
import { CURRENCIES, SOURCE_KINDS, sourceName } from "./rates.ts";

/** Most sources the node keeps. */
const MAX_SOURCES = 8;

export function RateSettings(): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const data = useNodeData();
  const id = createUniqueId();
  const [version, setVersion] = createSignal(0);
  const stored = createMemo(async () => {
    version();
    return (await client.call("getRateSources", {})).sources;
  });
  const [edited, setEdited] = createSignal<RateSource[]>();
  // Until the stored sources are read, there is nothing to edit: else a
  // source added would replace them all.
  const loaded = () => edited() ?? stored();
  const sources = () => loaded() ?? [];
  const [kind, setKind] = createSignal("coinbase");
  const [url, setUrl] = createSignal("");
  const [error, setError] = createSignal<string>();

  const chooseCurrency = async (currency: string) => {
    setError(undefined);
    try {
      await client.call("setDisplayCurrency", { currency });
      toast({ title: currency ? `Totals in ${currency}` : "No totals" });
    } catch (failure) {
      setError(describe(failure));
    }
  };
  const add = () => {
    if (sources().length >= MAX_SOURCES) {
      return setError(`At most ${MAX_SOURCES} sources.`);
    }
    const custom = kind() === "coinbase" ? url().trim() : "";
    setEdited([...sources(), { kind: kind(), url: custom } as RateSource]);
    setUrl("");
  };
  const remove = (index: number) =>
    setEdited(sources().filter((_, at) => at !== index));
  const save = async (event: SubmitEvent) => {
    event.preventDefault();
    setError(undefined);
    try {
      await client.call("setRateSources", {
        sources: sources().map(({ kind, url }) => ({ kind, url })),
      });
      toast({ title: "Rate sources saved" });
      setEdited(undefined);
      setVersion((n) => n + 1);
    } catch (failure) {
      setError(describe(failure));
    }
  };

  return (
    <Card title="Exchange Rates" testid="rate-settings">
      <Field label="Show totals in" for={`${id}-currency`}>
        <select
          id={`${id}-currency`}
          data-testid="display-currency"
          onChange={(event) => void chooseCurrency(event.currentTarget.value)}
        >
          <option value="" selected={data.displayCurrency() === ""}>
            No total
          </option>
          <For each={CURRENCIES}>
            {(code) => (
              <option value={code} selected={data.displayCurrency() === code}>
                {code}
              </option>
            )}
          </For>
        </select>
      </Field>
      <form class="stack" onSubmit={save}>
        <p class="muted">
          Rates come from these sources: each the median of those that give one,
          once more than half of them do.
        </p>
        <ul data-testid="rate-sources">
          <For each={sources()}>
            {(source, index) => (
              <li data-testid={`rate-source-${index()}`}>
                {sourceName(source.kind, source.url)}{" "}
                <button
                  type="button"
                  class="danger"
                  onClick={() => remove(index())}
                >
                  Remove
                </button>
              </li>
            )}
          </For>
        </ul>
        <Show when={sources().length === 0}>
          <p class="error" role="alert">
            Without a source, rates go stale, and payments across currencies
            stop.
          </p>
        </Show>
        <Field label="Add a source" for={`${id}-kind`}>
          <select
            id={`${id}-kind`}
            data-testid="rate-source-kind"
            onChange={(event) => setKind(event.currentTarget.value)}
          >
            <For each={SOURCE_KINDS}>
              {([value, label]) => (
                <option value={value} selected={kind() === value}>
                  {label}
                </option>
              )}
            </For>
          </select>
        </Field>
        <Show when={kind() === "coinbase"}>
          <Field
            label="URL"
            for={`${id}-url`}
            hint="Empty for Coinbase's own; else an HTTPS server of its format."
          >
            <input
              id={`${id}-url`}
              data-testid="rate-source-url"
              placeholder="https://api.coinbase.com/v2/exchange-rates?currency=USD"
              value={url()}
              onInput={(event) => setUrl(event.currentTarget.value)}
            />
          </Field>
        </Show>
        <div class="actions">
          <button
            type="button"
            data-testid="rate-source-add"
            disabled={loaded() === undefined}
            onClick={add}
          >
            Add
          </button>
          <button
            type="submit"
            class="primary"
            data-testid="rate-sources-save"
            disabled={edited() === undefined}
          >
            Save
          </button>
        </div>
      </form>
      <ErrorText message={error()} />
    </Card>
  );
}
