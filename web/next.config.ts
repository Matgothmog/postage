import type { NextConfig } from "next";

/// `NEXT_PUBLIC_WORLD_ENVIRONMENT` isn't just a domain switch: `world-id.ts`'s
/// `runSelfieCheck` (:243) reads it as a literal `NEXT_PUBLIC_*` access and
/// passes it straight into the IDKit request config, which travels inside
/// the *encrypted* bridge payload World's app decrypts - a build shipped with
/// "sandbox" still set would silently ask World's production verify endpoint
/// to accept a non-production proof, with nothing in the request/response
/// shape to flag the mismatch. Per this repo's Vercel GitHub integration, a
/// push to `origin/main` is live in roughly 35 seconds, so there's very
/// little standing between a stray local env var and real production
/// traffic - this is the last checkpoint before that happens.
///
/// Gated on `VERCEL_ENV === "production"` rather than `NODE_ENV`, because
/// `next build` sets `NODE_ENV=production` for *every* build, including an
/// ordinary one a developer runs locally while iterating on the sandbox flow
/// with `.env.local`'s `NEXT_PUBLIC_WORLD_ENVIRONMENT=sandbox` - that must
/// keep working. `VERCEL_ENV` (docs.vercel.com/environment-variables/system-
/// environment-variables) is set only by Vercel's own build system, and only
/// reads "production" for the build that deploys to production; a local
/// build, `next dev`, and a Vercel preview deploy all leave it unset or
/// "preview", so none of them trip this.
function assertWorldEnvironmentSafeForProduction(): void {
  if (process.env.VERCEL_ENV !== "production") return;

  const value = process.env.NEXT_PUBLIC_WORLD_ENVIRONMENT;
  if (!value || value === "production") return;

  throw new Error(
    `Refusing production build: NEXT_PUBLIC_WORLD_ENVIRONMENT is "${value}", expected ` +
      `"production" (or unset). This value is sent inside the encrypted IDKit bridge ` +
      `request to World, so shipping "${value}" to production would issue non-production ` +
      `Selfie Check proofs against World's production verify endpoint - silently, since ` +
      `nothing downstream would catch the mismatch.`
  );
}

assertWorldEnvironmentSafeForProduction();

/// Dev-only: Next 16 blocks cross-origin requests to dev resources (HMR
/// websocket, RSC payloads) by default, so loading /c/[token] from a phone
/// on the LAN never hydrates - React never attaches, the verify button goes
/// silently dead. `192.168.1.*` covers the whole local subnet, not just the
/// dev machine's current address, so it survives DHCP reassigning the
/// phone's IP. Matching is Next's own dot-segment wildcard (confirmed in
/// node_modules/next/dist/server/app-render/csrf-protection.js
/// matchWildcardDomain), the same mechanism used for host patterns like
/// "*.example.com" - it works on IPs because they're dot-separated too.
///
/// `*.trycloudflare.com` covers Cloudflare's ephemeral quick-tunnel
/// hostnames (e.g. https://<random-words>.trycloudflare.com), needed
/// because Privy's embedded wallet hard-throws off HTTPS on any origin
/// other than localhost/127.0.0.1 - a plain LAN IP over HTTP can't carry
/// the flow. The random subdomain is a single dot-free label, so one `*`
/// segment matches it exactly under matchWildcardDomain.
const nextConfig: NextConfig = {
  allowedDevOrigins: ["192.168.1.*", "*.trycloudflare.com"],
};

export default nextConfig;
