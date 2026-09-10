export const STALE_AFTER_MS = 6_000;
export const FUTURE_SKEW_MS = 60_000;

/** The same clock rule fences ordinary worker takeover and schema upgrades. */
export function stale(heartbeatAt: unknown, now: number): boolean {
  let ts: number;
  if (typeof heartbeatAt === 'number') ts = heartbeatAt;
  else if (typeof heartbeatAt === 'bigint') ts = Number(heartbeatAt);
  else ts = Number.NaN;
  return !Number.isFinite(ts) || now - ts > STALE_AFTER_MS || ts - now > FUTURE_SKEW_MS;
}
