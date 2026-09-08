/// Lets `node --test` load the app's modules unchanged.
///
/// The source imports relatives without an extension because that is what the
/// bundler expects; plain node ESM refuses them. Rather than write the imports
/// twice, this appends the extension when resolution would otherwise fail.
export async function resolve(specifier, context, next) {
  try {
    return await next(specifier, context);
  } catch (cause) {
    if (!specifier.startsWith(".")) throw cause;
    return next(`${specifier}.ts`, context);
  }
}
