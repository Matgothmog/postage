// Dev-only stand-in for /bridge/bridge.js, served by the e2e server in its
// place. Privy cannot run with dummy ids, so this installs the same bridge
// over the existing mock SDK (crates/web/tests/mock_sdk.js), pointed at the
// identities the server minted (window.__E2E, injected into the page).
//
// Who is "signed in" is test input, kept in localStorage:
//   e2e.persona   "owner" (default) | "sender"
//   e2e.signedIn  "1" once a login completed or the driver set a session
import { installBridge } from "/__e2e/bridge-core.js";
import { createMockSdk, mockCalls } from "/__e2e/mock_sdk.js";

const config = window.__E2E;
const read = (key) => {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
};
const write = (key, value) => {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private windows refuse storage; the session then lasts one page load.
  }
};

const persona = config.personas[read("e2e.persona") ?? "owner"];
const wallets = [{ address: persona.wallet, walletClientType: "privy" }];
const signedIn = {
  ready: true,
  authenticated: true,
  userId: persona.userId,
  email: persona.email,
  wallets,
  identityToken: persona.identityToken,
};
const signedOut = {
  ready: true,
  authenticated: false,
  userId: null,
  email: null,
  wallets: [],
  identityToken: null,
};
const startSignedIn = read("e2e.signedIn") === "1";

// The page reads the escrow straight from Arc's public RPC. Send those reads
// to the local node instead of the internet.
const realFetch = globalThis.fetch.bind(globalThis);
globalThis.fetch = (input, init) => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  if (url.startsWith("https://rpc.testnet.arc.network")) return realFetch(config.rpcUrl, init);
  return realFetch(input, init);
};

const sdk = createMockSdk(
  JSON.stringify({ snapshot: startSignedIn ? signedIn : signedOut, afterLogin: signedIn })
);
sdk.getIdentityToken = async () => (read("e2e.signedIn") === "1" ? persona.identityToken : null);

const mountMock = sdk.mountPrivy;
sdk.mountPrivy = (privyConfig, events) => {
  const mounted = mountMock(privyConfig, events);
  return {
    current() {
      const actions = mounted.current();
      if (!actions) return actions;
      return {
        ...actions,
        login() {
          write("e2e.signedIn", "1");
          return actions.login();
        },
        async logout() {
          write("e2e.signedIn", "0");
          return actions.logout();
        },
        // A real wallet would have put the payment on a chain the server
        // reads; here the local node is told, before the hash comes back.
        async sendTransaction(request) {
          const sent = await actions.sendTransaction(request);
          const noted = await realFetch(config.paymentUrl, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              from: persona.wallet,
              to: request.to,
              data: request.data,
              value: request.value === undefined ? null : request.value.toString(),
            }),
          });
          if (!noted.ok) throw new Error(`the local node refused the payment: ${noted.status}`);
          return sent;
        },
      };
    },
  };
};

window.__e2eCalls = () => JSON.parse(mockCalls());
installBridge(globalThis, sdk);
