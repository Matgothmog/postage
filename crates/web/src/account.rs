//! The signed-in home screen and the claim flow behind it. Replaces
//! `web/src/app/Account.tsx`.
//!
//! `Account` decides which of two things this address is: the page a stranger
//! reads (`landing`, rendered unchanged) or the inbox its owner manages. The
//! landing page is passed through, so nobody waits on a wallet library to read
//! a pitch.
//!
//! State lives in two `Copy` handles provided through context: `AccountState`
//! (what the server says about this wallet's inbox) and `ClaimFlow` (the claim
//! being made, from the hero's first keystroke to the live inbox).

use alloy_primitives::Address;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use postage_core::handle::is_valid_handle;
use postage_core::wallet_proof::read_statement;

use crate::api::{self, ClaimRequest, Inbox, InboxErrorKind};
use crate::bridge::privy::Privy;
use crate::chrome::{Callout, CalloutTone, QUIET_BUTTON, Shell};
use crate::claim_strip::{ClaimSetup, ClaimStrip};
use crate::inbox_panel::InboxPanel;
use crate::pending_claim::{
    ClaimProgress, PendingClaimOutcome, StoredClaim, as_store, claim_for, claim_store,
    clear_pending_claim, read_stored_claim, verify_pending_claim, write_pending_claim,
};
use crate::privy_context::{start_login, use_privy};
use crate::proof::{BrowserProofIo, sign_claim, suggest_handle, wallet_proof};

/// Which of the five things a signed-in page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Waiting,
    Error,
    Inbox,
    Strip,
    Setup,
}

/// The order the render picks between its five panels, as one readable list
/// that can be checked without a browser.
///
/// A claim in progress sits behind `inbox` rather than in front of it: the two
/// are never both true for long, and if they ever are, the finished inbox is
/// the truer answer.
///
/// Both sit in front of `error`, and that order is the fix. A refresh re-runs
/// whenever Privy rotates the identity token, and it never clears the inbox on
/// a failure, only sets the error. Deciding the error first meant one blip
/// replaced a live inbox, whose rows were still in state, with a full-screen
/// "Can't reach us.", and hid a claim halfway through behind an unrelated
/// fault. What we already read is still the truest thing we have; the failure
/// is a notice over it and a hard screen only when there is nothing behind it.
pub fn pick_panel(
    has_wallet: bool,
    loaded: bool,
    error: Option<InboxErrorKind>,
    has_inbox: bool,
    has_claim: bool,
) -> Panel {
    if !has_wallet || !loaded {
        return Panel::Waiting;
    }
    if has_inbox {
        return Panel::Inbox;
    }
    if has_claim {
        return Panel::Strip;
    }
    if error.is_some() {
        return Panel::Error;
    }
    Panel::Setup
}

/// What the server last said about this wallet's inbox.
#[derive(Debug, Clone, Copy)]
pub struct AccountState {
    privy: Privy,
    inbox: RwSignal<Option<Inbox>>,
    loaded: RwSignal<bool>,
    error: RwSignal<Option<InboxErrorKind>>,
    /// Counts reads started, so an answer that is no longer the latest (a
    /// slow one overtaken by a retry, or one for a wallet that has since
    /// signed out) is dropped instead of overwriting newer state.
    generation: StoredValue<u64>,
}

impl AccountState {
    /// Creates the state and starts reading whenever the wallet or the
    /// identity token changes. Call from the component that owns the account.
    pub fn new(privy: Privy) -> Self {
        let state = Self {
            privy,
            inbox: RwSignal::new(None),
            loaded: RwSignal::new(false),
            error: RwSignal::new(None),
            generation: StoredValue::new(0),
        };
        // One memo for both, so a snapshot that changes the wallet and the
        // token together re-reads once, not twice.
        let session = Memo::new(move |_| (privy.wallet().get(), privy.identity_token().get()));
        Effect::new(move |previous: Option<Option<Address>>| {
            let (wallet, _token) = session.get();
            // A new identity token re-reads with the same wallet; a different
            // wallet (or none) is a different account, and the previous one's
            // inbox must not be on screen while the new one loads.
            if previous.is_some_and(|previous| previous != wallet) {
                state.forget();
            }
            state.refresh();
            wallet
        });
        state
    }

    /// Drops everything read for the previous wallet, and any read still on
    /// its way for it.
    fn forget(&self) {
        self.generation.update_value(|generation| *generation += 1);
        self.inbox.set(None);
        self.loaded.set(false);
        self.error.set(None);
    }

    /// Reads the inbox again.
    pub fn refresh(&self) {
        let state = *self;
        spawn_local(async move { state.read().await });
    }

    /// The row holds the address the user actually reads, so the server will
    /// not hand it over on the strength of a wallet address alone, as those
    /// are public. Privy's identity token is that proof, already signed and in
    /// hand; signing a statement is the same proof made the long way, for a
    /// session that has no identity token to offer.
    ///
    /// A non-ok response is never treated as "no inbox": that would show
    /// someone who already claimed one the claim flow again. A refused proof
    /// needs a sign-in prompt; anything else means we could not ask at all,
    /// which needs different words and a retry.
    ///
    /// The previous error is cleared on the answer, not before it, because a
    /// request that has not answered yet says nothing about the last one. It
    /// has to be cleared there: a retry that succeeds for someone with no
    /// inbox sets the inbox to the `None` it already held, so leaving the
    /// error standing would pin them on the error screen and keep the claim
    /// flow out of reach for the rest of the session.
    ///
    /// A failure to produce the proof (a dismissed signature prompt, a nonce
    /// that could not be fetched) is "network" too: the same retry fixes it.
    async fn read(self) {
        let Some(wallet) = self.privy.wallet().get_untracked() else {
            return;
        };
        let Some(mine) = self.generation.try_update_value(|generation| {
            *generation += 1;
            *generation
        }) else {
            return;
        };
        let token = self.privy.identity_token().get_untracked();
        let io = BrowserProofIo::new(self.privy);
        let outcome = match wallet_proof(&io, token.as_deref(), wallet, |at| {
            read_statement(&wallet.to_string(), at)
        })
        .await
        {
            Ok(proof) => api::get_inbox(proof).await,
            Err(_) => Err(InboxErrorKind::Network),
        };

        if self.generation.try_get_value() != Some(mine) {
            return;
        }
        match outcome {
            Ok(inbox) => {
                self.inbox.try_set(inbox);
                self.error.try_set(None);
            }
            Err(kind) => {
                self.error.try_set(Some(kind));
            }
        }
        self.loaded.try_set(true);
    }
}

/// A claim the page has been asked to send. It is set from the click that
/// asks for it and never from an effect, so the button it came from goes busy
/// in the same render and the effect is only ever the sending.
///
/// `Hero` is the landing page's one-gesture ask. It names no destination
/// because there is no session yet to read one from, and it resolves to the
/// address Privy confirmed as soon as there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outgoing {
    Hero,
    Typed { handle: String, destination: String },
}

/// A claim that can be sent as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    handle: String,
    destination: String,
}

/// What an ask resolves to, or `None` when it cannot be sent as it stands. The
/// hero's cannot until Privy hands over an address: a passkey session never
/// gets one, so that user picks a destination in the setup panel instead, and
/// that interaction cannot be removed.
fn resolve_outgoing(
    outgoing: Option<&Outgoing>,
    handle: &str,
    email: Option<&str>,
) -> Option<Target> {
    let (handle, destination) = match outgoing? {
        Outgoing::Hero => (handle, email.unwrap_or_default()),
        Outgoing::Typed {
            handle,
            destination,
        } => (handle.as_str(), destination.as_str()),
    };
    let handle = handle.trim().to_lowercase();
    let destination = destination.trim().to_lowercase();
    (is_valid_handle(&handle) && destination.contains('@')).then_some(Target {
        handle,
        destination,
    })
}

/// Everything the claim needs to survive the moment it is made: the handle is
/// typed before there is a session, the POST cannot go out until there is one,
/// and the whole thing has to be findable again after a reload.
#[derive(Debug, Clone, Copy)]
pub struct ClaimFlow {
    privy: Privy,
    account: AccountState,
    /// What the user actually typed, or `None` while they have typed nothing.
    /// Kept apart from what is shown so the suggestion can fill the field once
    /// Privy hands over an address (which happens after the hero has already
    /// been filled in) without overwriting anybody's own answer.
    typed_handle: RwSignal<Option<String>>,
    typed_destination: RwSignal<Option<String>>,
    /// The storage slot as it was found, owner and all. Read when the flow is
    /// created rather than from an effect, so a reload while waiting on
    /// Cloudflare shows the strip straight away instead of flashing the claim
    /// form at somebody who has already filled it in, and so the hero's queued
    /// claim cannot go out before the claim already on record has been read
    /// back.
    record: RwSignal<Option<StoredClaim>>,
    outgoing: RwSignal<Option<Outgoing>>,
    error: RwSignal<Option<String>>,
    /// Whether a request is already on its way: it must not be sent twice.
    sending: StoredValue<bool>,
    /// The claim that came back from storage (only that one, never a claim
    /// this page has just made) is worth checking with the server once per
    /// page load.
    checked: StoredValue<bool>,
    restored: StoredValue<Option<StoredClaim>>,
    /// The claim in hand, which is only ever the one this wallet stored. A
    /// claim left behind by whoever used this browser last is neither shown
    /// nor acted on.
    progress: Memo<Option<ClaimProgress>>,
    panel: Memo<Panel>,
    handle: Memo<String>,
    destination: Memo<String>,
    target: Memo<Option<Target>>,
    busy: Memo<bool>,
}

impl ClaimFlow {
    pub fn new(privy: Privy, account: AccountState) -> Self {
        let record = read_stored_claim(as_store(&claim_store()));
        let typed_handle = RwSignal::new(None::<String>);
        let typed_destination = RwSignal::new(None::<String>);
        let outgoing = RwSignal::new(None::<Outgoing>);

        let wallet_text = Memo::new(move |_| privy.wallet().get().map(|w| w.to_string()));
        let record_signal = RwSignal::new(record.clone());
        let progress = Memo::new(move |_| {
            record_signal.with(|record| claim_for(record.as_ref(), wallet_text.get().as_deref()))
        });
        let panel = Memo::new(move |_| {
            pick_panel(
                privy.wallet().get().is_some(),
                account.loaded.get(),
                account.error.get(),
                account.inbox.with(Option::is_some),
                progress.with(Option::is_some),
            )
        });
        let handle = Memo::new(move |_| {
            typed_handle
                .get()
                .unwrap_or_else(|| suggest_handle(privy.email().get().as_deref()))
        });
        let destination = Memo::new(move |_| {
            typed_destination
                .get()
                .or_else(|| privy.email().get())
                .unwrap_or_default()
        });
        let target = Memo::new(move |_| {
            outgoing.with(|outgoing| {
                resolve_outgoing(
                    outgoing.as_ref(),
                    &handle.get(),
                    privy.email().get().as_deref(),
                )
            })
        });

        let flow = Self {
            privy,
            account,
            typed_handle,
            typed_destination,
            record: record_signal,
            outgoing,
            error: RwSignal::new(None),
            sending: StoredValue::new(false),
            checked: StoredValue::new(false),
            restored: StoredValue::new(record),
            progress,
            panel,
            handle,
            destination,
            target,
            busy: Memo::new(move |_| target.with(Option::is_some)),
        };
        flow.send_when_the_account_can_take_it();
        flow.reconcile_a_restored_claim();
        flow
    }

    pub fn handle(&self) -> Memo<String> {
        self.handle
    }

    pub fn destination(&self) -> Memo<String> {
        self.destination
    }

    pub fn email(&self) -> Memo<Option<String>> {
        self.privy.email()
    }

    pub fn error(&self) -> RwSignal<Option<String>> {
        self.error
    }

    pub fn progress(&self) -> Memo<Option<ClaimProgress>> {
        self.progress
    }

    pub fn panel(&self) -> Memo<Panel> {
        self.panel
    }

    pub fn privy_ready(&self) -> Memo<bool> {
        self.privy.ready()
    }

    /// Whether the claim is being sent: true once an ask resolves to
    /// something sendable, until the answer is in.
    pub fn busy(&self) -> Memo<bool> {
        self.busy
    }

    pub fn set_handle(&self, handle: String) {
        self.typed_handle.set(Some(handle));
    }

    pub fn set_destination(&self, destination: String) {
        self.typed_destination.set(Some(destination));
    }

    /// The hero's one gesture: remember the handle, then open the sign-in that
    /// the claim needs before it can be sent.
    pub fn start(&self) {
        self.error.set(None);
        self.outgoing.set(Some(Outgoing::Hero));
        start_login(self.privy);
    }

    pub fn submit(&self) {
        self.error.set(None);
        self.outgoing.set(Some(Outgoing::Typed {
            handle: self.handle.get_untracked(),
            destination: self.destination.get_untracked(),
        }));
    }

    pub fn advance(&self, next: ClaimProgress) {
        let Some(wallet) = self.privy.wallet().get_untracked() else {
            return;
        };
        let wallet = wallet.to_string();
        write_pending_claim(as_store(&claim_store()), Some(&wallet), &next);
        self.record.set(Some(StoredClaim {
            wallet,
            claim: next,
        }));
    }

    /// The way out of a claim, wherever it is used from: the strip's own
    /// control, a poll that comes back saying the server has dropped it, and
    /// signing out.
    pub fn restart(&self) {
        clear_pending_claim(as_store(&claim_store()));
        self.record.set(None);
        self.error.set(None);
    }

    pub fn finish(&self) {
        clear_pending_claim(as_store(&claim_store()));
        self.record.set(None);
        self.account.refresh();
    }

    /// The hero's button carried the handle and the sign-in together, so the
    /// claim it asked for goes out the moment the session can answer for it.
    /// A second click here would be asking the user to say the same thing
    /// twice.
    ///
    /// It goes out from one state only, and `pick_panel` already names it: a
    /// loaded account, with a wallet, no inbox, no claim in progress and no
    /// failed read behind it. `loaded` alone says the request finished, not
    /// that it answered: a failed read leaves it true with no inbox, which
    /// reads exactly like a wallet with no inbox, and the claim used to go out
    /// behind the error screen. That spends one of the five a wallet gets in
    /// an hour somewhere its own error can never be read. The ask is kept
    /// rather than dropped, so a retry that succeeds sends it.
    fn send_when_the_account_can_take_it(self) {
        Effect::new(move |_| {
            let target = self.target.get();
            let panel = self.panel.get();
            let Some(target) = target else {
                return;
            };
            if panel != Panel::Setup || self.sending.get_value() {
                return;
            }
            self.sending.set_value(true);
            spawn_local(self.send(target));
        });
    }

    async fn send(self, target: Target) {
        let outcome = self.post(&target).await;
        match outcome {
            Ok(Some(started)) => {
                if let Some(wallet) = self.privy.wallet().try_get_untracked().flatten() {
                    let wallet = wallet.to_string();
                    write_pending_claim(as_store(&claim_store()), Some(&wallet), &started);
                    self.record.try_set(Some(StoredClaim {
                        wallet,
                        claim: started,
                    }));
                }
            }
            Ok(None) => {
                clear_pending_claim(as_store(&claim_store()));
                self.record.try_set(None);
                self.account.refresh();
            }
            Err(message) => {
                self.error.try_set(Some(message));
            }
        }
        self.sending.try_set_value(false);
        self.outgoing.try_set(None);
    }

    /// Starts the claim server-side. `Ok(None)` means it went live at once
    /// (the destination was the verified sign-in address); `Ok(Some)` is a
    /// claim waiting on its confirmations.
    async fn post(self, target: &Target) -> Result<Option<ClaimProgress>, String> {
        let wallet = self
            .privy
            .wallet()
            .get_untracked()
            .ok_or_else(|| "Sign in again and retry".to_owned())?;
        let io = BrowserProofIo::new(self.privy);
        let token = self.privy.identity_token().get_untracked();
        let proof = sign_claim(&io, &target.handle, &target.destination, wallet).await;
        if token.is_none() && proof.is_none() {
            return Err(
                "Could not confirm the wallet is yours. Sign in again and retry".to_owned(),
            );
        }
        let request = ClaimRequest {
            handle: target.handle.clone(),
            destination: target.destination.clone(),
            wallet,
        };
        let reply = api::post_claim(token.as_deref(), &request, proof.as_ref())
            .await
            .map_err(|error| error.to_string())?;
        if reply.live {
            return Ok(None);
        }
        Ok(Some(ClaimProgress {
            handle: reply.handle,
            destination: reply.destination,
            code_verified: reply.code_verified,
            cloudflare_verified: reply.cloudflare_verified,
        }))
    }

    /// Claim progress used to live in component state alone, so a reload while
    /// waiting on Cloudflare's email dropped the claimer back onto the empty
    /// form, and claiming again spent one of the five a wallet gets in an
    /// hour. What was stored is shown first and reconciled here, against the
    /// endpoint the strip already polls: an answer we cannot get is not a
    /// reason to forget a claim.
    fn reconcile_a_restored_claim(self) {
        Effect::new(move |_| {
            let loaded = self.account.loaded.get();
            let wallet = self.privy.wallet().get().map(|wallet| wallet.to_string());
            let has_inbox = self.account.inbox.with(Option::is_some);
            // Only ever this wallet's own. Somebody else's is not checked with
            // the server, not cleared, and not shown: it is simply not ours to
            // touch.
            let restored = self.restored.get_value();
            let Some(stored) = claim_for(restored.as_ref(), wallet.as_deref()) else {
                return;
            };
            if !loaded || self.checked.get_value() {
                return;
            }
            self.checked.set_value(true);

            // The inbox is already real, so the stored claim is a leftover of
            // the one that made it. The panel shows the inbox over it either
            // way; this stops it outliving the session.
            if has_inbox {
                clear_pending_claim(as_store(&claim_store()));
                return;
            }
            let Some(wallet) = wallet else {
                return;
            };
            spawn_local(self.settle_restored(wallet, stored));
        });
    }

    async fn settle_restored(self, wallet: String, stored: ClaimProgress) {
        let outcome = verify_pending_claim(&stored).await;
        // Whatever the user did while this was in flight (started over, signed
        // out) wins: an answer about a claim that is no longer on screen must
        // not put it back.
        let still_held = self
            .record
            .try_with_untracked(|record| {
                record
                    .as_ref()
                    .is_some_and(|held| held.wallet == wallet && held.claim.handle == stored.handle)
            })
            .unwrap_or(false);
        match outcome {
            PendingClaimOutcome::Live => {
                clear_pending_claim(as_store(&claim_store()));
                self.record.try_set(None);
                self.account.refresh();
            }
            PendingClaimOutcome::Gone => {
                clear_pending_claim(as_store(&claim_store()));
                self.record.try_set(None);
            }
            PendingClaimOutcome::Pending(claim) if still_held => {
                self.record.try_set(Some(StoredClaim { wallet, claim }));
            }
            // "Unknown" leaves what was stored on screen; the strip's own poll
            // asks again four seconds later.
            PendingClaimOutcome::Pending(_) | PendingClaimOutcome::Unknown => {}
        }
    }
}

/// The page at `/`.
#[component]
pub fn Account(landing: ViewFn) -> impl IntoView {
    let privy = use_privy();
    let account = AccountState::new(privy);
    let flow = ClaimFlow::new(privy, account);
    provide_context(flow);
    provide_context(account);

    let signed_in = Memo::new(move |_| privy.ready().get() && privy.authenticated().get());
    move || {
        if signed_in.get() {
            view! {
                <Shell actions=SignedInActions>
                    <SignedInBody />
                </Shell>
            }
            .into_any()
        } else {
            let landing = landing.clone();
            view! { <Shell actions=SignedOutActions>{landing.run()}</Shell> }.into_any()
        }
    }
}

#[component]
fn SignedOutActions() -> impl IntoView {
    let privy = use_privy();
    view! {
        <A href="/network" attr:class=QUIET_BUTTON>
            "Ledger"
        </A>
        <button
            type="button"
            class=QUIET_BUTTON
            disabled=move || !privy.ready().get()
            on:click=move |_| start_login(privy)
        >
            "Sign in"
        </button>
    }
}

#[component]
fn SignedInActions() -> impl IntoView {
    let privy = use_privy();
    let flow = use_context::<ClaimFlow>();
    view! {
        <A href="/network" attr:class=QUIET_BUTTON>
            "Ledger"
        </A>
        <button
            type="button"
            class=QUIET_BUTTON
            on:click=move |_| {
                // One browser, one storage slot. A claim left in it is the
                // claim the next person to sign in on this machine would be
                // shown: handle, destination address and all.
                if let Some(flow) = flow {
                    flow.restart();
                }
                spawn_local(async move {
                    if let Err(error) = privy.logout().await {
                        leptos::logging::error!("sign-out failed: {error}");
                    }
                });
            }
        >
            "Sign out"
        </button>
    }
}

/// The five panels, and the notice over two of them.
#[component]
fn SignedInBody() -> impl IntoView {
    let privy = use_privy();
    let (Some(account), Some(flow)) = (use_context::<AccountState>(), use_context::<ClaimFlow>())
    else {
        return ().into_any();
    };
    let retry = Callback::new(move |()| account.refresh());
    let panel = flow.panel();
    let inbox = Memo::new(move |_| account.inbox.get());
    let wallet = privy.wallet();

    let notice = move || {
        let behind_something = matches!(panel.get(), Panel::Inbox | Panel::Strip);
        account
            .error
            .get()
            .filter(|_| behind_something)
            .map(|kind| view! { <RefreshFailed kind on_retry=retry /> })
    };

    let body = move || match panel.get() {
        Panel::Error => match account.error.get() {
            Some(kind) => view! { <InboxError kind on_retry=retry /> }.into_any(),
            None => view! { <Waiting /> }.into_any(),
        },
        Panel::Inbox => match (inbox.get(), wallet.get()) {
            (Some(inbox), Some(wallet)) => view! { <InboxPanel inbox wallet /> }.into_any(),
            _ => view! { <Waiting /> }.into_any(),
        },
        Panel::Strip => match (flow.progress().get(), wallet.get()) {
            (Some(_), Some(wallet)) => view! {
                <ClaimStrip
                    claim=Signal::derive(move || flow.progress().get().unwrap_or_default())
                    wallet
                    on_claim=Callback::new(move |next| flow.advance(next))
                    on_live=Callback::new(move |()| flow.finish())
                    on_restart=Callback::new(move |()| flow.restart())
                />
            }
            .into_any(),
            _ => view! { <Waiting /> }.into_any(),
        },
        Panel::Setup => view! {
            <ClaimSetup
                handle=flow.handle()
                on_handle=Callback::new(move |handle| flow.set_handle(handle))
                destination=flow.destination()
                on_destination=Callback::new(move |destination| flow.set_destination(destination))
                email=flow.email()
                busy=flow.busy()
                error=flow.error()
                on_submit=Callback::new(move |()| flow.submit())
            />
        }
        .into_any(),
        Panel::Waiting => view! { <Waiting /> }.into_any(),
    };

    view! {
        {notice}
        {body}
    }
    .into_any()
}

#[component]
fn Waiting() -> impl IntoView {
    view! { <main class="m-auto px-6 py-32 text-center text-sm text-faint">"One moment."</main> }
}

/// `title`/`body` are the words for a page with nothing on it. `stale` is the
/// same fault with something real already on screen, where the news is not
/// that we failed but that what is showing is a moment behind.
struct ErrorCopy {
    title: &'static str,
    body: &'static str,
    stale: &'static str,
}

fn error_copy(kind: InboxErrorKind) -> ErrorCopy {
    match kind {
        InboxErrorKind::Unauthorized => ErrorCopy {
            title: "That’s not your wallet.",
            body: "Sign out and back in.",
            stale: "Your session went stale. Sign out and back in to refresh this.",
        },
        InboxErrorKind::Network => ErrorCopy {
            title: "Can’t reach us.",
            body: "Try again.",
            stale: "We couldn’t reach the server just now.",
        },
    }
}

/// A refresh that failed with something already on screen. It replaces
/// nothing: the inbox or the claim below it is real, came from this same
/// server, and a rotated identity token or a dropped connection is not a
/// reason to take it away and offer a retry in its place. `InboxError` is the
/// same fault with nothing behind it, which is the only time a screen of its
/// own is honest.
#[component]
fn RefreshFailed(kind: InboxErrorKind, on_retry: Callback<()>) -> impl IntoView {
    let copy = error_copy(kind);
    view! {
        <div class="mx-auto w-full max-w-3xl px-6 pt-6">
            <Callout tone=CalloutTone::Bad title="Showing what we last read.">
                <p>{copy.stale}</p>
                <button
                    type="button"
                    on:click=move |_| on_retry.run(())
                    class=format!("{QUIET_BUTTON} -ml-3 mt-1 text-bad")
                >
                    "Try again"
                </button>
            </Callout>
        </div>
    }
}

#[component]
fn InboxError(kind: InboxErrorKind, on_retry: Callback<()>) -> impl IntoView {
    let copy = error_copy(kind);
    view! {
        <main class="rise mx-auto w-full max-w-xl px-6 py-16">
            <Callout tone=CalloutTone::Bad title=copy.title>
                <p>{copy.body}</p>
                <button
                    type="button"
                    on:click=move |_| on_retry.run(())
                    class=format!("{QUIET_BUTTON} -ml-3 mt-1 text-bad")
                >
                    "Try again"
                </button>
            </Callout>
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NETWORK: Option<InboxErrorKind> = Some(InboxErrorKind::Network);
    const UNAUTHORIZED: Option<InboxErrorKind> = Some(InboxErrorKind::Unauthorized);

    #[test]
    fn pick_panel_keeps_the_precedence_the_render_depends_on_wallet_and_first_load_first() {
        assert_eq!(pick_panel(false, false, None, false, false), Panel::Waiting);
        assert_eq!(pick_panel(false, true, NETWORK, true, true), Panel::Waiting);
        assert_eq!(pick_panel(true, false, None, true, true), Panel::Waiting);
        assert_eq!(pick_panel(true, true, NETWORK, false, false), Panel::Error);
        assert_eq!(
            pick_panel(true, true, None, true, true),
            Panel::Inbox,
            "a finished inbox beats a claim still on record for it"
        );
        assert_eq!(pick_panel(true, true, None, false, true), Panel::Strip);
        assert_eq!(pick_panel(true, true, None, false, false), Panel::Setup);
    }

    #[test]
    fn an_inbox_already_read_survives_a_failed_refresh_error_or_not() {
        assert_eq!(
            pick_panel(true, true, NETWORK, true, false),
            Panel::Inbox,
            "a transient network error must not take a live inbox off the screen"
        );
        assert_eq!(
            pick_panel(true, true, UNAUTHORIZED, true, false),
            Panel::Inbox,
            "a rotated identity token must not take a live inbox off the screen either"
        );
        assert_eq!(
            pick_panel(true, true, NETWORK, true, true),
            Panel::Inbox,
            "the inbox still wins over a claim still on record for it"
        );
    }

    #[test]
    fn a_claim_in_progress_is_not_hidden_behind_an_unrelated_network_error() {
        assert_eq!(pick_panel(true, true, NETWORK, false, true), Panel::Strip);
    }

    #[test]
    fn the_error_screen_is_still_what_a_failed_read_with_nothing_behind_it_shows() {
        assert_eq!(pick_panel(true, true, NETWORK, false, false), Panel::Error);
        assert_eq!(
            pick_panel(true, true, UNAUTHORIZED, false, false),
            Panel::Error
        );
    }

    /// The send gate is the panel itself: setup is the one state a claim goes
    /// out from, so "we do not know" can no longer pass for "no inbox".
    #[test]
    fn a_claim_is_never_sent_while_the_account_read_is_the_thing_that_failed() {
        assert_ne!(pick_panel(true, true, NETWORK, false, false), Panel::Setup);
        assert_ne!(
            pick_panel(true, true, UNAUTHORIZED, false, false),
            Panel::Setup
        );
        assert_ne!(pick_panel(true, false, None, false, false), Panel::Setup);
        assert_ne!(pick_panel(false, true, None, false, false), Panel::Setup);
        assert_eq!(
            pick_panel(true, true, None, false, false),
            Panel::Setup,
            "an account read that answered with no inbox is the one state a claim goes out from"
        );
    }

    #[test]
    fn the_heros_ask_waits_for_an_address_from_privy_and_a_valid_handle() {
        let hero = Some(Outgoing::Hero);
        assert_eq!(resolve_outgoing(hero.as_ref(), "demo", None), None);
        assert_eq!(resolve_outgoing(hero.as_ref(), "d", Some("a@b.c")), None);
        assert_eq!(
            resolve_outgoing(hero.as_ref(), " Demo ", Some("Me@Example.com")),
            Some(Target {
                handle: "demo".to_owned(),
                destination: "me@example.com".to_owned()
            })
        );
        assert_eq!(resolve_outgoing(None, "demo", Some("a@b.c")), None);
    }

    #[test]
    fn a_typed_ask_uses_what_was_typed_not_the_session_email() {
        let typed = Some(Outgoing::Typed {
            handle: "typed".to_owned(),
            destination: "Other@Example.com".to_owned(),
        });
        assert_eq!(
            resolve_outgoing(typed.as_ref(), "ignored", Some("me@example.com")),
            Some(Target {
                handle: "typed".to_owned(),
                destination: "other@example.com".to_owned()
            })
        );
        let no_at = Some(Outgoing::Typed {
            handle: "typed".to_owned(),
            destination: "nope".to_owned(),
        });
        assert_eq!(resolve_outgoing(no_at.as_ref(), "x", None), None);
    }

    #[test]
    fn each_failure_kind_has_words_for_a_bare_page_and_for_a_notice() {
        for kind in [InboxErrorKind::Unauthorized, InboxErrorKind::Network] {
            let copy = error_copy(kind);
            assert!(!copy.title.is_empty() && !copy.body.is_empty() && !copy.stale.is_empty());
        }
        assert_eq!(error_copy(InboxErrorKind::Network).title, "Can’t reach us.");
    }
}
