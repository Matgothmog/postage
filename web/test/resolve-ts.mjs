import { pathToFileURL } from "node:url";
import { join } from "node:path";

const SRC = join(process.cwd(), "src");

/// Lets `node --test` load the app's modules unchanged.
///
/// Two things plain node ESM will not do that the bundler does: resolve a
/// relative import with no extension, and understand the `@/` alias. Rewriting
/// the imports to suit the test runner would mean the tests no longer exercise
/// what ships, so they are resolved here instead.
export async function resolve(specifier, context, next) {
  if (specifier.startsWith("@/")) {
    return next(`${pathToFileURL(join(SRC, specifier.slice(2))).href}.ts`, context);
  }
  try {
    return await next(specifier, context);
  } catch (cause) {
    if (!specifier.startsWith(".")) throw cause;
    return next(`${specifier}.ts`, context);
  }
}
