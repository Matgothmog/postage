/// Seconds, which is what every timestamp column here holds. This was written
/// out by hand twenty-one times, and one `Date.now()` among them would have
/// stored milliseconds into a column the next query reads as seconds — an
/// expiry fifty thousand years out that nothing would ever report.
export function now(): number {
  return Math.floor(Date.now() / 1000);
}
