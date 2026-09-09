/// The gateway's decision about one inbound message, exactly as it crosses the
/// wire between the two packages that agree on it: `web`'s `/api/mail/inbound`
/// route answers every request with this shape, and `worker` parses the
/// response body as this shape and acts on it — forwarding, holding, or
/// rejecting the message the answer names.
///
/// Declared once, here, and referenced by relative path from both `web/src` and
/// `worker/src`. `web/` and `worker/` are separate npm packages with their own
/// tsconfigs and no workspace tooling between them, so there is no package one
/// side could import the other's types from without adding one; a type-only
/// file reached by a plain relative import is the only route to a single
/// declaration that does not do that. Type-only, so nothing about a request
/// path or a build step is added on either side by depending on it: an
/// `import type` is erased before either package runs.
export interface GatewayVerdict {
  action: "forward" | "hold" | "reject";
  /// Verified destination, on `forward` only.
  to?: string;
  reason?: string;
  /// Key the message is held under, on `hold` only.
  token?: string;
  held_until?: number;
  /// Absent when answering the sender would mean mailing someone whose name was
  /// forged, in which case the SMTP refusal carries the link instead.
  notice?: GatewayNotice | null;
  /// What to say inside the SMTP session if we do not write back.
  bounce?: string;
}

/// The one message a held sender is written back, carried inside a `hold`
/// verdict. `web/src/lib/challenge-email.ts` builds the value that fills this;
/// it is not imported here because a `@/*`-aliased module is only resolvable
/// inside `web/`, so the shape is restated structurally instead.
export interface GatewayNotice {
  subject: string;
  html: string;
  text: string;
}
