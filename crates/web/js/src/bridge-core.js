// The surface the Rust web app calls, built over whichever SDK objects it is
// handed. Imports nothing, so the wasm-bindgen tests load this exact file with
// a mocked SDK while the shipped bundle (entry.js) hands it the real Privy
// island and IDKit.
//
// Every value crossing into Rust is plain JSON text, and every rejection is a
// `{code, message}` object, so the Rust side parses one shape and never has to
// inspect a JS Error.

/// Where the bundle publishes the bridge for the Rust side to find.
export const BRIDGE_GLOBAL = "__postageBridge";

/// Turns whatever an SDK threw into the one error shape Rust understands.
/// Privy errors carry `privyErrorCode`, wallets an EIP-1193 numeric `code`
/// (4001 when the user rejects), IDKit and Privy callbacks a bare code
/// string; anything else keeps at least its message.
export function toBridgeError(cause, fallbackCode = "sdk_error") {
  if (typeof cause === "string") return { code: cause, message: cause };
  if (cause && typeof cause === "object") {
    const code = errorCodeOf(cause) ?? fallbackCode;
    const message = typeof cause.message === "string" && cause.message ? cause.message : code;
    return { code, message };
  }
  return { code: fallbackCode, message: String(cause) };
}

function errorCodeOf(cause) {
  if (typeof cause.privyErrorCode === "string") return cause.privyErrorCode;
  if (typeof cause.code === "string") return cause.code;
  if (typeof cause.code === "number") return String(cause.code);
  return null;
}

function rejectWith(cause, fallbackCode) {
  return Promise.reject(toBridgeError(cause, fallbackCode));
}

function createPrivyBridge(sdk) {
  let island = null;
  let pendingLogin = null;

  function settleLogin(outcome) {
    const pending = pendingLogin;
    pendingLogin = null;
    if (!pending) return;
    if (outcome.ok) pending.resolve();
    else pending.reject(toBridgeError(outcome.code, "login_failed"));
  }

  function actions() {
    const current = island ? island.current() : null;
    if (!current) throw { code: "not_ready", message: "Privy has not finished loading" };
    return current;
  }

  return {
    /// Mounts Privy once. `configJson` is the serialized Rust `PrivyConfig`;
    /// `onState` receives every auth snapshot as JSON text.
    start(configJson, onState) {
      if (island) throw { code: "already_started", message: "Privy is already started" };
      const config = JSON.parse(configJson);
      island = sdk.mountPrivy(config, {
        snapshot: (state) => onState(JSON.stringify(state)),
        loginComplete: () => settleLogin({ ok: true }),
        loginError: (code) => settleLogin({ ok: false, code }),
      });
    },

    /// Opens the Privy modal; settles when the flow completes or the user
    /// leaves it. A second call supersedes the first, which rejects.
    login() {
      let open;
      try {
        open = actions().login;
      } catch (cause) {
        return rejectWith(cause, "not_ready");
      }
      if (pendingLogin) {
        pendingLogin.reject({ code: "superseded", message: "A newer login replaced this one" });
      }
      const settled = new Promise((resolve, reject) => {
        pendingLogin = { resolve, reject };
      });
      try {
        open();
      } catch (cause) {
        settleLogin({ ok: false, code: toBridgeError(cause, "login_failed").code });
      }
      return settled;
    },

    async logout() {
      try {
        await actions().logout();
      } catch (cause) {
        throw toBridgeError(cause, "logout_failed");
      }
    },

    /// Resolves to the signature hex string.
    async signMessage(message, address) {
      try {
        const { signature } = await actions().signMessage(
          { message },
          { address, uiOptions: { showWalletUIs: false } }
        );
        return signature;
      } catch (cause) {
        throw toBridgeError(cause, "sign_failed");
      }
    },

    /// `txJson` is `{to, data, value?, chainId}` with `value` as a decimal
    /// string, since a JS number cannot hold a wei amount. Resolves to the hash.
    async sendTransaction(txJson) {
      try {
        const tx = JSON.parse(txJson);
        const request = { to: tx.to, data: tx.data, chainId: tx.chainId };
        if (typeof tx.value === "string") request.value = BigInt(tx.value);
        const { hash } = await actions().sendTransaction(request);
        return hash;
      } catch (cause) {
        throw toBridgeError(cause, "send_failed");
      }
    },

    /// The freshest identity token Privy can give, or null when signed out.
    async identityToken() {
      try {
        const token = await sdk.getIdentityToken();
        return typeof token === "string" ? token : null;
      } catch (cause) {
        throw toBridgeError(cause, "identity_token_failed");
      }
    },
  };
}

function createIdkitBridge(sdk) {
  return {
    /// `requestJson` is the serialized Rust `SelfieCheckRequest`. Resolves to
    /// a handle whose `pollJson` never rejects: it resolves to the completion
    /// as JSON text, an SDK throw folded into a `generic_error` failure.
    async openSelfieCheck(requestJson) {
      let handle;
      try {
        const { config, signal } = JSON.parse(requestJson);
        handle = await sdk.IDKit.request(config).preset(sdk.selfieCheckLegacy({ signal }));
      } catch (cause) {
        throw toBridgeError(cause, "request_failed");
      }
      return {
        connectorURI: handle.connectorURI,
        async pollJson(timeoutMs) {
          try {
            const completion = await handle.pollUntilCompletion({ timeout: timeoutMs });
            return JSON.stringify(completion);
          } catch (cause) {
            console.error("Selfie Check polling rejected unexpectedly", cause);
            return JSON.stringify({ success: false, error: "generic_error" });
          }
        },
      };
    },
  };
}

/// `sdk` is `{mountPrivy, getIdentityToken, IDKit, selfieCheckLegacy}`.
export function createBridge(sdk) {
  return { privy: createPrivyBridge(sdk), idkit: createIdkitBridge(sdk) };
}

export function installBridge(target, sdk) {
  target[BRIDGE_GLOBAL] = createBridge(sdk);
  return target[BRIDGE_GLOBAL];
}
