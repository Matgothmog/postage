// A stand-in for the SDK object `js/src/entry.js` hands to `installBridge`:
// the same shape as the real Privy island and IDKit, scripted, and recording
// every call so the tests can check what crossed the boundary. No network.

let calls = [];

export function mockCalls() {
  return JSON.stringify(calls);
}

/// `optionsJson`: `{ready?: bool, loginError?: string}`.
export function createMockSdk(optionsJson) {
  const options = JSON.parse(optionsJson);
  calls = [];

  function mountPrivy(config, events) {
    calls.push({ fn: "mountPrivy", appId: config.appId, chainId: config.client.defaultChain.id });
    const ready = options.ready !== false;
    const actions = {
      login() {
        calls.push({ fn: "login" });
        queueMicrotask(() => {
          if (options.loginError) events.loginError(options.loginError);
          else events.loginComplete();
        });
      },
      async logout() {
        calls.push({ fn: "logout" });
      },
      async signMessage(input, signOptions) {
        calls.push({ fn: "signMessage", input, options: signOptions });
        if (input.message === "reject") {
          const error = new Error("User rejected request");
          error.code = 4001;
          throw error;
        }
        return { signature: "0xsigned" };
      },
      async sendTransaction(request) {
        calls.push({
          fn: "sendTransaction",
          to: request.to,
          data: request.data,
          chainId: request.chainId,
          valueType: typeof request.value,
          value: request.value === undefined ? null : request.value.toString(),
        });
        return { hash: `0x${"ab".repeat(32)}` };
      },
    };
    queueMicrotask(() =>
      events.snapshot({
        ready,
        authenticated: true,
        userId: "did:privy:test",
        email: "Tester@Example.com",
        wallets: [{ address: "0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7", walletClientType: "privy" }],
        identityToken: "id-token-1",
      })
    );
    return { current: () => (ready ? actions : null) };
  }

  const IDKit = {
    request(config) {
      return {
        async preset(preset) {
          calls.push({ fn: "idkit", config, preset });
          if (preset.signal === "throw") throw new Error("IDKit could not start");
          return {
            connectorURI: "https://world.org/verify?t=mock",
            async pollUntilCompletion({ timeout }) {
              calls.push({ fn: "poll", timeout });
              if (preset.signal === "rejected") return { success: false, error: "user_rejected" };
              if (preset.signal === "poll-throws") throw new Error("broke its word");
              return { success: true, result: { protocol_version: "3.0", nullifier: "0x01" } };
            },
          };
        },
      };
    },
  };

  return {
    mountPrivy,
    getIdentityToken: async () => "id-token-fresh",
    IDKit,
    selfieCheckLegacy: ({ signal }) => ({ type: "selfie_check_legacy", signal }),
  };
}
