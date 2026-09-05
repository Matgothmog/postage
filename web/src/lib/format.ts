import { formatUnits, parseUnits } from "viem";
import { USDC_DECIMALS } from "./contracts";

/// Postage is small enough that the useful unit is cents, not dollars.
export function formatUsdc(amount: bigint): string {
  const value = Number(formatUnits(amount, USDC_DECIMALS));
  if (value === 0) return "free";
  if (value < 0.01) return `${(value * 100).toFixed(2)}c`;
  return `$${value.toFixed(2)}`;
}

export function parseUsdc(input: string): bigint {
  return parseUnits(input.trim(), USDC_DECIMALS);
}

export function shortAddress(address: string): string {
  return `${address.slice(0, 6)}...${address.slice(-4)}`;
}
