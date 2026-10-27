import { describe, expect, test } from "bun:test";

import { formatCost, formatUsd, isDecimalString, sumCosts } from "../src/util/money";

/**
 * Cost formatting must match the Rust ledger exactly
 * (`src/domain/money.rs::format_usd`: round to 2 dp, half away from zero).
 */
describe("formatUsd", () => {
  test("matches the Rust baseline cases", () => {
    expect(formatUsd("0.0049")).toBe("0.00");
    expect(formatUsd("0.1250")).toBe("0.13");
    expect(formatUsd("1.2")).toBe("1.20");
    expect(formatUsd("-0.005")).toBe("-0.01");
  });

  test("keeps two decimals and does not use floats", () => {
    expect(formatUsd("0")).toBe("0.00");
    expect(formatUsd("0.1")).toBe("0.10");
    expect(formatUsd("12")).toBe("12.00");
    expect(formatUsd("1234.567")).toBe("1234.57");
  });

  test("rounds half away from zero in both directions", () => {
    expect(formatUsd("0.005")).toBe("0.01");
    expect(formatUsd("0.015")).toBe("0.02");
    expect(formatUsd("2.675")).toBe("2.68");
    expect(formatUsd("-2.675")).toBe("-2.68");
  });

  test("never renders a negative zero", () => {
    expect(formatUsd("-0.001")).toBe("0.00");
  });

  test("passes non-decimal input through unchanged", () => {
    expect(formatUsd("unknown")).toBe("unknown");
    expect(formatUsd("")).toBe("");
  });
});

describe("sumCosts", () => {
  test("sums decimal strings exactly", () => {
    expect(sumCosts(["0.000123", "0.004500"])).toBe("0.00");
    expect(sumCosts(["0.5", "0.25"])).toBe("0.75");
    expect(sumCosts(["1.005", "0.005"])).toBe("1.01");
  });

  test("ignores unknown values but reports them as unknown overall", () => {
    expect(sumCosts([null, undefined, ""])).toBeNull();
    expect(sumCosts(["0.10", null])).toBe("0.10");
  });

  test("handles values with different scales", () => {
    expect(sumCosts(["0.1", "0.02"])).toBe("0.12");
    expect(sumCosts(["1.005", "2.5"])).toBe("3.51");
  });
});

describe("helpers", () => {
  test("isDecimalString validates ledger values", () => {
    expect(isDecimalString("0.000123")).toBe(true);
    expect(isDecimalString("-12")).toBe(true);
    expect(isDecimalString("1e5")).toBe(false);
    expect(isDecimalString("abc")).toBe(false);
  });

  test("formatCost tolerates null", () => {
    expect(formatCost(null)).toBe("");
    expect(formatCost("0.5")).toBe("0.50");
  });
});
