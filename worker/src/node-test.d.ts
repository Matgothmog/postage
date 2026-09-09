/// Node's test runner and assert module, narrowed to what the tests here call.
///
/// This worker runs on workerd, not Node, and `tsconfig.json` pins `types` to
/// `@cloudflare/workers-types` so that nothing in `src/` can reach for a Node
/// global the deployed runtime does not have. Installing `@types/node` would
/// undo exactly that: it puts `process`, `Buffer` and Node's timers back in
/// scope for the shipped code to typecheck against. The tests are the only
/// place that touches Node at all, and only for its runner, so the runner is
/// declared here instead.
///
/// Deliberately under-typed. Every signature is narrower than the real one, so
/// it can reject a valid call - fix the declaration if that ever happens - but
/// never lets an invalid one through.

declare module "node:test" {
  export function test(name: string, fn: () => void | Promise<void>): Promise<void>;
  export function beforeEach(fn: () => void | Promise<void>): void;
  export function afterEach(fn: () => void | Promise<void>): void;
}

declare module "node:assert/strict" {
  interface StrictAssert {
    (value: unknown, message?: string): void;
    ok(value: unknown, message?: string): void;
    equal(actual: unknown, expected: unknown, message?: string): void;
    deepEqual(actual: unknown, expected: unknown, message?: string): void;
    match(value: string, pattern: RegExp, message?: string): void;
  }
  const assert: StrictAssert;
  export default assert;
}
