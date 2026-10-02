// Bundle entry: wires the real SDKs into the bridge and publishes it on
// `globalThis` before the Rust app starts (index.html loads this module ahead
// of the wasm loader, and module scripts run in document order).
//
// Privy only ships its login modal, embedded wallet and signing prompts as
// React, so it lives here as an island: one hidden React root holding just
// PrivyProvider and a component that reports hook state out and keeps the
// hook functions reachable. No app UI is rendered by React.

import { createElement, useEffect } from "react";
import { createRoot } from "react-dom/client";
import {
  PrivyProvider,
  getIdentityToken,
  useIdentityToken,
  useLogin,
  usePrivy,
  useSendTransaction,
  useSignMessage,
  useWallets,
} from "@privy-io/react-auth";
import { IDKit, selfieCheckLegacy } from "@worldcoin/idkit-core";
import { installBridge } from "./bridge-core.js";

function snapshotOf({ ready, authenticated, user, wallets, identityToken }) {
  return {
    ready,
    authenticated,
    userId: user?.id ?? null,
    email: user?.email?.address ?? null,
    wallets: wallets.map((wallet) => ({
      address: wallet.address,
      walletClientType: wallet.walletClientType ?? null,
    })),
    identityToken: identityToken ?? null,
  };
}

function Island({ events, publish }) {
  const { ready, authenticated, user, logout } = usePrivy();
  const { wallets } = useWallets();
  const { identityToken } = useIdentityToken();
  const { signMessage } = useSignMessage();
  const { sendTransaction } = useSendTransaction();
  const { login } = useLogin({
    onComplete: () => events.loginComplete(),
    onError: (code) => events.loginError(code),
  });

  // Hook functions can change identity between renders; the bridge must
  // always call the latest ones.
  publish({ login: () => login(), logout, signMessage, sendTransaction });

  const walletKey = wallets.map((wallet) => wallet.address).join(",");
  const email = user?.email?.address ?? null;
  useEffect(() => {
    events.snapshot(snapshotOf({ ready, authenticated, user, wallets, identityToken }));
    // `user` and `wallets` are new objects on most renders; their identity
    // is not a change worth reporting, the keys below are.
  }, [ready, authenticated, user?.id, email, walletKey, identityToken]);

  return null;
}

function mountPrivy(config, events) {
  let latest = null;
  const host = document.createElement("div");
  host.id = "privy-island";
  document.body.appendChild(host);
  createRoot(host).render(
    createElement(
      PrivyProvider,
      { appId: config.appId, config: config.client },
      createElement(Island, { events, publish: (actions) => (latest = actions) })
    )
  );
  return { current: () => latest };
}

installBridge(globalThis, { mountPrivy, getIdentityToken, IDKit, selfieCheckLegacy });
