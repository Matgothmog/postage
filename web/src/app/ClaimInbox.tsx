"use client";

import { useState } from "react";

export function ClaimInbox({ wallet, onClaimed }: { wallet: string; onClaimed: () => void }) {
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  async function claim() {
    setSaving(true);
    setError(null);
    try {
      const response = await fetch("/api/inbox/claim", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ localPart: name.trim().toLowerCase(), wallet }),
      });
      const result = (await response.json()) as { error?: string };
      if (!response.ok) throw new Error(result.error ?? "Could not claim that name");
      onClaimed();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="mt-8 rounded-xl border border-neutral-200 bg-white p-5">
      <h2 className="text-sm font-medium">Pick your address</h2>
      <p className="mt-1 text-sm text-neutral-600">
        Mail sent here is held until the sender proves they are a person or attaches postage.
      </p>
      <div className="mt-4 flex items-center gap-2">
        <input
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder="you"
          className="w-40 rounded-lg border border-neutral-300 px-3 py-2 text-sm"
        />
        <span className="text-sm text-neutral-500">@usepostage.com</span>
        <button
          onClick={claim}
          disabled={saving || name.trim().length < 2}
          className="ml-auto rounded-lg bg-neutral-900 px-4 py-2 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
        >
          {saving ? "Claiming" : "Claim"}
        </button>
      </div>
      {error && <p className="mt-3 text-sm text-red-600">{error}</p>}
    </section>
  );
}
