/// A `catch` clause hands back `unknown`, not `Error` — anything can be
/// thrown, and code that assumes otherwise breaks the moment something
/// throws a string or a plain object instead. This is the one place that
/// decision is made, so every caller gets the same answer. Mirrors
/// `causeMessage` in `worker/src/index.ts`, the same fix for the same
/// problem on the other side of the repo with no runtime sharing between
/// them.
///
/// Framework-agnostic on purpose: it is imported from both `"use client"`
/// components and server route handlers, so nothing here may pull in
/// anything Node-only.
export function causeMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
