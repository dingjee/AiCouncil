/**
 * Exact money handling.
 *
 * The Rust ledger stores decimal fixed-point values and formats them with
 * `Decimal::round_dp_with_strategy(2, MidpointAwayFromZero)` (see
 * `src/domain/money.rs::format_usd`). The UI must display the **exact same
 * string**, so:
 *
 * - costs that cross the IPC boundary are already formatted strings and are
 *   printed verbatim;
 * - `formatUsd` reproduces the Rust rounding for the rare case where the
 *   frontend has to format a raw decimal itself. It works on the decimal
 *   *string* with integer arithmetic, never on a float.
 */

const DECIMAL_RE = /^-?\d+(\.\d+)?$/;

export function isDecimalString(value: string): boolean {
  return DECIMAL_RE.test(value.trim());
}

/**
 * Round a decimal string to 2 places, half away from zero.
 *
 * Mirrors Rust: `formatUsd("0.0049") === "0.00"`, `formatUsd("0.1250") ===
 * "0.13"`, `formatUsd("1.2") === "1.20"`, `formatUsd("-0.005") === "-0.01"`.
 *
 * Rounding "away from zero" means: the magnitude grows whenever the first
 * dropped digit is 5 or more (an exact `.5` midpoint rounds away, anything
 * above the midpoint also rounds up).
 */
export function formatUsd(raw: string): string {
  const value = raw.trim();
  if (!isDecimalString(value)) return value;

  const negative = value.startsWith("-");
  const unsigned = negative ? value.slice(1) : value;
  const [intPart, fracPart = ""] = unsigned.split(".");

  const digits = fracPart.padEnd(3, "0");
  const third = digits.charCodeAt(2) - 48;

  let scaled = BigInt(intPart + digits.slice(0, 2));
  if (third >= 5) scaled += 1n;

  const text = scaled.toString().padStart(3, "0");
  const whole = text.slice(0, text.length - 2);
  const cents = text.slice(text.length - 2);
  const out = `${whole}.${cents}`;
  // Never render a negative zero; a magnitude that rounds to zero is zero.
  if (negative && /^0\.00$/.test(out)) return out;
  return negative ? `-${out}` : out;
}

/** Format a raw decimal for display, tolerating null/undefined. */
export function formatCost(raw: string | null | undefined): string {
  if (raw === null || raw === undefined || raw === "") return "";
  return formatUsd(raw);
}

/**
 * Sum exact decimal strings and format the result.
 *
 * Used only for values the backend did not pre-aggregate. Returns `null` when
 * no input is known, mirroring the backend rule that "unknown is never zero".
 */
export function sumCosts(values: (string | null | undefined)[]): string | null {
  let total = 0n;
  let scale = 0;
  let seen = false;

  for (const value of values) {
    if (value === null || value === undefined || value === "") continue;
    const trimmed = value.trim();
    if (!isDecimalString(trimmed)) continue;
    seen = true;

    const negative = trimmed.startsWith("-");
    const [intPart, fracPart = ""] = trimmed.replace(/^-/, "").split(".");
    const currentScale = fracPart.length;

    // Align every value to the widest scale seen so far.
    const targetScale = Math.max(scale, currentScale);
    const digits = intPart + fracPart + "0".repeat(targetScale - currentScale);
    let scaled = BigInt(digits);
    if (negative) scaled = -scaled;

    if (targetScale > scale) {
      total *= 10n ** BigInt(targetScale - scale);
      scale = targetScale;
    }
    total += scaled;
  }
  if (!seen) return null;

  const negative = total < 0n;
  const abs = negative ? -total : total;
  const divisor = 10n ** BigInt(scale);
  const whole = abs / divisor;
  const frac = abs % divisor;
  const fracText = frac.toString().padStart(scale, "0");
  const plain = `${whole.toString()}${scale ? "." + fracText : ""}`;
  return formatUsd(`${negative ? "-" : ""}${plain}`);
}

/** Render a token count with thousands separators. */
export function formatTokens(count: number | null | undefined): string {
  if (count === null || count === undefined) return "—";
  return count.toLocaleString("en-US");
}
