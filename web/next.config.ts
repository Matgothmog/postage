import type { NextConfig } from "next";

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
