import { useEffect, useMemo, useState } from "react";
import type { LocalizationBundle } from "../../domain/types";
import { formatByteValues, formatDates, formatNumbers } from "../../i18n";

type Formatter<T> = (values: T[]) => Promise<string[]>;

function useFormattedValues<T extends string | number>(
  values: T[],
  locale: string,
  formatter: Formatter<T>,
  fallback: (value: T) => string,
) {
  const serializedValues = JSON.stringify(values);
  const requestKey = `${locale}:${serializedValues}`;
  const [result, setResult] = useState<{ key: string; values: string[] }>();

  useEffect(() => {
    let active = true;
    const requestedValues = JSON.parse(serializedValues) as T[];
    void formatter(requestedValues)
      .then((formatted) => {
        if (active) setResult({ key: requestKey, values: formatted });
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [formatter, requestKey, serializedValues]);

  return result?.key === requestKey ? result.values : values.map(fallback);
}

export function localizedNumber(
  localization: LocalizationBundle,
  value: number,
) {
  return Number.isInteger(value) &&
    value >= 0 &&
    value < localization.numbers.length
    ? localization.numbers[value]
    : String(value);
}

export function useLocalizedNumbers(
  values: number[],
  localization: LocalizationBundle,
) {
  return useFormattedValues(
    values,
    localization.locale,
    formatNumbers,
    (value) => localizedNumber(localization, value),
  );
}

export function useLocalizedDates(
  values: Array<string | undefined>,
  localization: LocalizationBundle,
) {
  const sources = useMemo(() => values.map((value) => value ?? ""), [values]);
  return useFormattedValues(
    sources,
    localization.locale,
    formatDates,
    (value) => value,
  );
}

export function useLocalizedBytes(
  values: Array<number | undefined>,
  localization: LocalizationBundle,
) {
  const defined = values.map((value) => value ?? 0);
  return useFormattedValues(
    defined,
    localization.locale,
    formatByteValues,
    (value) => `${value} B`,
  );
}
